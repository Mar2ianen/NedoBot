mod render;
mod repo;

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use teloxide::{
    Bot,
    prelude::*,
    types::{ChatId, ChatMemberKind, Message, User},
};

pub use render::render_report;
pub use repo::{ReportCard, ReportResolution};

const MAX_REASON_CHARS: usize = 500;
const MAX_REPORTS_PER_WINDOW: i64 = 1;
const REPORT_WINDOW_MINUTES: i64 = 10;
const DELIVERY_LEASE_SECONDS: i64 = 10 * 60;
const MAX_DELIVERY_ATTEMPTS: i32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportCreation {
    Created(i64),
    AlreadyExists(i64),
    RateLimited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportAction {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportActionResult {
    Applied(ReportResolution),
    AlreadyResolved,
    Missing,
}

#[derive(Debug, Clone)]
pub struct ReportTarget {
    pub chat_id: i64,
    pub message_id: i32,
    pub reporter_user_id: i64,
    pub reported_user_id: i64,
    pub reason: String,
    pub target_text: Option<String>,
    pub target_media: String,
    pub target_reply_to_message_id: Option<i32>,
    pub target_created_at: DateTime<Utc>,
    pub reporter_snapshot: Value,
    pub target_snapshot: Value,
}

#[derive(Debug, Clone, Copy)]
struct ReportDelivery {
    report_id: i64,
    admin_user_id: i64,
    attempt_count: i32,
}

pub fn target_from_reply(msg: &Message, reason: &str) -> Option<ReportTarget> {
    let reporter = msg.from.as_ref()?;
    let target = msg.reply_to_message()?;
    let target_user = target.from.as_ref()?;
    Some(ReportTarget {
        chat_id: msg.chat.id.0,
        message_id: target.id.0,
        reporter_user_id: reporter.id.0 as i64,
        reported_user_id: target_user.id.0 as i64,
        reason: normalize_reason(reason),
        target_text: crate::telegram::entities::message_text(target).map(str::to_owned),
        target_media: media_kind(target),
        target_reply_to_message_id: target.reply_to_message().map(|message| message.id.0),
        target_created_at: target.date,
        reporter_snapshot: user_snapshot(reporter),
        target_snapshot: user_snapshot(target_user),
    })
}

pub fn normalize_reason(reason: &str) -> String {
    let reason = reason.trim();
    if reason.chars().count() <= MAX_REASON_CHARS {
        return reason.to_owned();
    }
    reason
        .chars()
        .take(MAX_REASON_CHARS - 1)
        .chain(['…'])
        .collect()
}

pub fn media_kind(message: &Message) -> String {
    let mut kinds = Vec::new();
    if message.photo().is_some() {
        kinds.push("фото");
    }
    if message.video().is_some() {
        kinds.push("видео");
    }
    if message.document().is_some() {
        kinds.push("документ");
    }
    if message.audio().is_some() {
        kinds.push("аудио");
    }
    if message.voice().is_some() {
        kinds.push("голосовое");
    }
    if message.sticker().is_some() {
        kinds.push("стикер");
    }
    if message.animation().is_some() {
        kinds.push("GIF");
    }
    if kinds.is_empty() {
        "текст".to_owned()
    } else {
        kinds.join(", ")
    }
}

fn user_snapshot(user: &User) -> Value {
    json!({
        "id": user.id.0,
        "username": user.username,
        "first_name": user.first_name,
        "last_name": user.last_name,
        "is_bot": user.is_bot,
        "is_premium": user.is_premium,
        "language_code": user.language_code,
    })
}

pub async fn resolve_admin_ids(
    bot: &Bot,
    pool: &PgPool,
    config: &crate::config::Config,
) -> anyhow::Result<Vec<i64>> {
    let mut admin_ids = Vec::new();
    match bot
        .get_chat_administrators(ChatId(config.discussion_chat_id))
        .await
    {
        Ok(members) => {
            admin_ids.extend(
                members
                    .into_iter()
                    .filter(|member| !member.user.is_bot)
                    .map(|member| member.user.id.0 as i64),
            );
        }
        Err(err) => {
            tracing::warn!(%err, "failed to resolve live chat administrators for /report");
        }
    }

    if admin_ids.is_empty() {
        let rows = sqlx::query(
            r#"
            select telegram_user_id
            from telegram_chat_member_snapshots
            where chat_id = $1 and is_admin and is_present
            union
            select telegram_user_id
            from telegram_chat_users
            where chat_id = $1 and is_admin and coalesce(is_present, true)
            "#,
        )
        .bind(config.discussion_chat_id)
        .fetch_all(pool)
        .await?;
        admin_ids.extend(
            rows.into_iter()
                .map(|row| row.get::<i64, _>("telegram_user_id")),
        );
    }

    if let Some(owner_id) = config.owner_telegram_id {
        admin_ids.push(owner_id);
    }
    admin_ids.sort_unstable();
    admin_ids.dedup();
    Ok(admin_ids)
}

pub async fn create_report(
    pool: &PgPool,
    target: &ReportTarget,
    admin_ids: &[i64],
) -> anyhow::Result<ReportCreation> {
    let mut tx = pool.begin().await?;
    sqlx::query("select pg_advisory_xact_lock($1::bigint)")
        .bind(target.reporter_user_id)
        .execute(&mut *tx)
        .await?;

    let existing_id: Option<i64> = sqlx::query_scalar(
        "select id from telegram_reports where chat_id = $1 and message_id = $2",
    )
    .bind(target.chat_id)
    .bind(target.message_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(existing_id) = existing_id {
        tx.commit().await?;
        return Ok(ReportCreation::AlreadyExists(existing_id));
    }

    let recent_count: i64 = sqlx::query_scalar(
        r#"
        select count(*)::bigint
        from telegram_reports
        where reporter_user_id = $1
          and created_at >= now() - ($2 * interval '1 minute')
        "#,
    )
    .bind(target.reporter_user_id)
    .bind(REPORT_WINDOW_MINUTES)
    .fetch_one(&mut *tx)
    .await?;
    if recent_count >= MAX_REPORTS_PER_WINDOW {
        return Ok(ReportCreation::RateLimited);
    }

    let report_id = sqlx::query_scalar::<_, i64>(
        r#"
        insert into telegram_reports (
            chat_id, message_id, reporter_user_id, reported_user_id, reason,
            target_text, target_media, target_reply_to_message_id,
            target_created_at, reporter_snapshot, target_snapshot
        ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        on conflict (chat_id, message_id) do nothing
        returning id
        "#,
    )
    .bind(target.chat_id)
    .bind(target.message_id)
    .bind(target.reporter_user_id)
    .bind(target.reported_user_id)
    .bind(&target.reason)
    .bind(&target.target_text)
    .bind(&target.target_media)
    .bind(target.target_reply_to_message_id)
    .bind(target.target_created_at)
    .bind(&target.reporter_snapshot)
    .bind(&target.target_snapshot)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(report_id) = report_id else {
        let existing_id: i64 = sqlx::query_scalar(
            "select id from telegram_reports where chat_id = $1 and message_id = $2",
        )
        .bind(target.chat_id)
        .bind(target.message_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(ReportCreation::AlreadyExists(existing_id));
    };

    for admin_id in admin_ids.iter().copied() {
        sqlx::query(
            "insert into telegram_report_deliveries (report_id, admin_user_id) values ($1, $2)",
        )
        .bind(report_id)
        .bind(admin_id)
        .execute(&mut *tx)
        .await?;
    }
    if admin_ids.is_empty() {
        sqlx::query(
            "update telegram_reports set status = 'failed', updated_at = now() where id = $1",
        )
        .bind(report_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(ReportCreation::Created(report_id))
}

async fn claim_next_delivery(pool: &PgPool) -> anyhow::Result<Option<ReportDelivery>> {
    let row = sqlx::query(
        r#"
        with candidate as (
            select delivery.report_id, delivery.admin_user_id
            from telegram_report_deliveries delivery
            join telegram_reports report on report.id = delivery.report_id
            where delivery.attempt_count < $1
              and (
                  (delivery.status in ('pending', 'failed') and delivery.next_attempt_at <= now())
                  or (delivery.status = 'processing' and delivery.lease_expires_at <= now())
              )
              and report.resolution = 'pending'
            order by delivery.next_attempt_at, delivery.report_id, delivery.admin_user_id
            for update of delivery skip locked
            limit 1
        )
        update telegram_report_deliveries delivery
        set status = 'processing',
            attempt_count = delivery.attempt_count + 1,
            lease_expires_at = now() + ($2 * interval '1 second'),
            updated_at = now()
        from candidate
        where delivery.report_id = candidate.report_id
          and delivery.admin_user_id = candidate.admin_user_id
        returning delivery.report_id, delivery.admin_user_id, delivery.attempt_count
        "#,
    )
    .bind(MAX_DELIVERY_ATTEMPTS)
    .bind(DELIVERY_LEASE_SECONDS)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| ReportDelivery {
        report_id: row.get("report_id"),
        admin_user_id: row.get("admin_user_id"),
        attempt_count: row.get("attempt_count"),
    }))
}

pub async fn process_next_delivery(bot: &Bot, pool: &PgPool) -> anyhow::Result<bool> {
    let Some(delivery) = claim_next_delivery(pool).await? else {
        return Ok(false);
    };
    let report = repo::load_report(pool, delivery.report_id).await?;
    let message = bot
        .send_rich_message(ChatId(delivery.admin_user_id), render_report(&report))
        .await;
    match message {
        Ok(message) => {
            mark_delivery_sent(pool, &delivery, message.id.0).await?;
        }
        Err(err) => {
            let failure = classify_delivery_error(&err, delivery.attempt_count);
            mark_delivery_failed(pool, &delivery, failure).await?;
            if failure.retryable() {
                tracing::warn!(
                    %err,
                    report_id = delivery.report_id,
                    admin_user_id = delivery.admin_user_id,
                    attempt = delivery.attempt_count,
                    "report delivery failed; will retry"
                );
            } else {
                tracing::warn!(
                    %err,
                    report_id = delivery.report_id,
                    admin_user_id = delivery.admin_user_id,
                    "report delivery is not reachable"
                );
            }
        }
    }
    update_report_status(pool, delivery.report_id).await?;
    Ok(true)
}

#[derive(Debug, Clone, Copy)]
struct DeliveryFailure {
    error_kind: &'static str,
    retryable: bool,
    delay_seconds: i64,
}

impl DeliveryFailure {
    const fn retryable(self) -> bool {
        self.retryable
    }
}

fn classify_delivery_error(error: &teloxide::RequestError, attempt: i32) -> DeliveryFailure {
    if let teloxide::RequestError::RetryAfter(seconds) = error {
        return DeliveryFailure {
            error_kind: "telegram_retry_after",
            retryable: true,
            delay_seconds: i64::from(seconds.seconds()),
        };
    }
    let message = error.to_string().to_lowercase();
    if message.contains("forbidden")
        || message.contains("chat not found")
        || message.contains("user is deactivated")
    {
        return DeliveryFailure {
            error_kind: "telegram_unreachable",
            retryable: false,
            delay_seconds: 0,
        };
    }
    DeliveryFailure {
        error_kind: if attempt >= MAX_DELIVERY_ATTEMPTS {
            "telegram_retry_exhausted"
        } else {
            "telegram_send_failed"
        },
        retryable: attempt < MAX_DELIVERY_ATTEMPTS,
        delay_seconds: 2_i64.pow(attempt.min(8) as u32).min(300),
    }
}

async fn mark_delivery_sent(
    pool: &PgPool,
    delivery: &ReportDelivery,
    message_id: i32,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update telegram_report_deliveries
        set status = 'sent', telegram_message_id = $3, lease_expires_at = null,
            error_kind = null, updated_at = now()
        where report_id = $1 and admin_user_id = $2 and status = 'processing'
        "#,
    )
    .bind(delivery.report_id)
    .bind(delivery.admin_user_id)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_delivery_failed(
    pool: &PgPool,
    delivery: &ReportDelivery,
    failure: DeliveryFailure,
) -> anyhow::Result<()> {
    let status = if failure.retryable {
        "failed"
    } else {
        "unreachable"
    };
    sqlx::query(
        r#"
        update telegram_report_deliveries
        set status = $3, next_attempt_at = now() + ($4 * interval '1 second'),
            lease_expires_at = null, error_kind = $5, updated_at = now()
        where report_id = $1 and admin_user_id = $2 and status = 'processing'
        "#,
    )
    .bind(delivery.report_id)
    .bind(delivery.admin_user_id)
    .bind(status)
    .bind(failure.delay_seconds)
    .bind(failure.error_kind)
    .execute(pool)
    .await?;
    Ok(())
}

async fn update_report_status(pool: &PgPool, report_id: i64) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update telegram_reports report
        set status = case
            when totals.total = totals.sent then 'sent'
            when totals.sent > 0 then 'partial'
            when totals.active > 0 then 'pending'
            else 'failed'
        end,
        updated_at = now()
        from (
            select report_id,
                   count(*) as total,
                   count(*) filter (where status = 'sent') as sent,
                   count(*) filter (where status in ('pending', 'processing', 'failed')) as active
            from telegram_report_deliveries
            where report_id = $1
            group by report_id
        ) totals
        where report.id = totals.report_id
        "#,
    )
    .bind(report_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub fn parse_callback(data: &str) -> Option<(i64, ReportAction)> {
    let mut parts = data.split(':');
    if parts.next()? != "report" {
        return None;
    }
    let report_id = parts.next()?.parse::<i64>().ok()?;
    if report_id <= 0 {
        return None;
    }
    let action = match parts.next()? {
        "accept" => ReportAction::Accept,
        "reject" => ReportAction::Reject,
        _ => return None,
    };
    (parts.next().is_none()).then_some((report_id, action))
}

pub async fn is_admin_callback_actor(
    bot: &Bot,
    config: &crate::config::Config,
    user_id: i64,
) -> bool {
    if user_id <= 0 {
        return false;
    }
    if config.owner_telegram_id == Some(user_id) {
        return true;
    }
    bot.get_chat_member(
        ChatId(config.discussion_chat_id),
        teloxide::types::UserId(user_id as u64),
    )
    .await
    .map(|member| {
        matches!(
            member.kind,
            ChatMemberKind::Administrator(_) | ChatMemberKind::Owner(_)
        )
    })
    .unwrap_or(false)
}

pub async fn apply_action(
    pool: &PgPool,
    report_id: i64,
    action: ReportAction,
    actor_id: i64,
) -> anyhow::Result<ReportActionResult> {
    let resolution = match action {
        ReportAction::Accept => ReportResolution::Accepted,
        ReportAction::Reject => ReportResolution::Rejected,
    };
    let status = resolution.as_str();
    let rows = sqlx::query(
        r#"
        update telegram_reports
        set resolution = $2, resolved_by_user_id = $3, resolved_at = now(), updated_at = now()
        where id = $1 and resolution = 'pending'
        "#,
    )
    .bind(report_id)
    .bind(status)
    .bind(actor_id)
    .execute(pool)
    .await?;
    if rows.rows_affected() == 0 {
        let exists: Option<(String,)> =
            sqlx::query_as("select resolution from telegram_reports where id = $1")
                .bind(report_id)
                .fetch_optional(pool)
                .await?;
        return Ok(if exists.is_some() {
            ReportActionResult::AlreadyResolved
        } else {
            ReportActionResult::Missing
        });
    }
    Ok(ReportActionResult::Applied(resolution))
}

pub async fn load_report(pool: &PgPool, report_id: i64) -> anyhow::Result<ReportCard> {
    repo::load_report(pool, report_id).await
}

pub fn report_target_context(msg: &Message, config: &crate::config::Config) -> anyhow::Result<()> {
    if msg.chat.id.0 != config.discussion_chat_id {
        anyhow::bail!("/report is available only in the discussion chat")
    }
    let target = msg
        .reply_to_message()
        .context("/report must be a reply to a user message")?;
    let Some(user) = msg.from.as_ref() else {
        anyhow::bail!("reporter identity is unavailable")
    };
    let Some(target_user) = target.from.as_ref() else {
        anyhow::bail!("target user identity is unavailable")
    };
    if user.is_bot || target_user.is_bot || target.is_automatic_forward() {
        anyhow::bail!("only human user messages can be reported")
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_reason_to_the_storage_limit() {
        let value = normalize_reason(&"я".repeat(MAX_REASON_CHARS + 10));
        assert_eq!(value.chars().count(), MAX_REASON_CHARS);
        assert!(value.ends_with('…'));
    }

    #[test]
    fn parses_only_report_callbacks() {
        assert_eq!(
            parse_callback("report:42:accept"),
            Some((42, ReportAction::Accept))
        );
        assert_eq!(parse_callback("spam_review:42:spam"), None);
        assert_eq!(parse_callback("report:0:accept"), None);
    }

    #[test]
    fn enforces_reporter_rate_limit_contract() {
        assert_eq!(MAX_REPORTS_PER_WINDOW, 1);
        assert_eq!(REPORT_WINDOW_MINUTES, 10);
    }

    #[test]
    fn snapshots_are_json_objects() {
        let value = user_snapshot(&User {
            id: teloxide::types::UserId(42),
            is_bot: false,
            first_name: "Test".to_owned(),
            last_name: None,
            username: None,
            language_code: None,
            is_premium: false,
            added_to_attachment_menu: false,
            supports_guest_queries: false,
            supports_join_request_queries: false,
            has_topics_enabled: false,
            allows_users_to_create_topics: false,
            can_manage_bots: false,
        });
        assert_eq!(value["id"], json!(42));
    }
}
