use serde_json::Value;
use sqlx::{PgPool, Row};
use teloxide::{
    prelude::*,
    types::{
        InlineKeyboardButton, InlineKeyboardMarkup, LinkPreviewOptions, MessageId, ParseMode,
        ReplyParameters,
    },
};

use crate::{
    config_file::ModerationConfig,
    features::{
        jobs::{claim::CasResult, policy::ANALYSIS_RETRY},
        labels::{
            LabelSource, SpamLabel, record_not_spam_in_transaction, record_spam_in_transaction,
        },
    },
    telegram::html,
};

const DELIVERY_LEASE_SECONDS: i64 = 10 * 60;
/// Максимальный возраст первого сообщения для Telegram-доставки ревью.
/// Просроченный first message остаётся в аудите, но карточка не claim-ится:
/// администраторов не стоит будить ради давно остывшего контекста.
pub(crate) const FIRST_MESSAGE_REVIEW_MAX_AGE_SECONDS: i64 = 5 * 60;

pub struct SpamReview {
    pub id: i64,
    pub chat_id: i64,
    pub destination_chat_id: i64,
    pub first_message_id: Option<i32>,
    pub notification_message_id: Option<i32>,
    pub notification_attempts: i32,
    pub notification_consecutive_failures: i32,
    pub risk_score: i32,
    pub review_threshold: i32,
    pub risk_signals: Value,
    pub text: String,
}

#[allow(dead_code)] // Остаётся ручным/integration API; runtime materializer пишет review атомарно.
pub async fn create_review(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
) -> anyhow::Result<Option<SpamReview>> {
    let request_id = sqlx::query_scalar(
        r#"
        insert into spam_review_requests (chat_id, telegram_user_id, risk_score, risk_signals)
        select a.chat_id, a.telegram_user_id, a.risk_score, a.risk_signal_breakdown
        from telegram_new_user_profile_audits a
        where a.chat_id = $1 and a.telegram_user_id = $2
        on conflict (chat_id, telegram_user_id) do update
        set risk_score = excluded.risk_score,
            risk_signals = excluded.risk_signals,
            notification_status = case
                when spam_review_requests.status = 'pending'
                 and spam_review_requests.notification_status in ('pending', 'retry_wait', 'sent')
                 and (spam_review_requests.notified_risk_score, spam_review_requests.notified_risk_signals)
                     is distinct from (excluded.risk_score, excluded.risk_signals)
                    then 'retry_wait'
                else spam_review_requests.notification_status
            end,
            notification_next_attempt_at = case
                when spam_review_requests.status = 'pending'
                 and spam_review_requests.notification_status in ('pending', 'retry_wait', 'sent')
                 and (spam_review_requests.notified_risk_score, spam_review_requests.notified_risk_signals)
                     is distinct from (excluded.risk_score, excluded.risk_signals)
                    then now()
                else spam_review_requests.notification_next_attempt_at
            end,
            notification_error_kind = case
                when spam_review_requests.status = 'pending'
                 and spam_review_requests.notification_status in ('pending', 'retry_wait', 'sent')
                 and (spam_review_requests.notified_risk_score, spam_review_requests.notified_risk_signals)
                     is distinct from (excluded.risk_score, excluded.risk_signals)
                    then null
                else spam_review_requests.notification_error_kind
            end,
            notification_consecutive_failures = case
                when spam_review_requests.status = 'pending'
                 and spam_review_requests.notification_status in ('pending', 'retry_wait', 'sent')
                 and (spam_review_requests.notified_risk_score, spam_review_requests.notified_risk_signals)
                     is distinct from (excluded.risk_score, excluded.risk_signals)
                    then 0
                else spam_review_requests.notification_consecutive_failures
            end
        returning id
        "#,
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(request_id) = request_id else {
        return Ok(None);
    };
    claim_review_delivery(pool, Some(request_id), None).await
}

#[allow(dead_code)] // Compatibility API for callers that use the legacy source-chat destination.
pub async fn claim_next_review_delivery(pool: &PgPool) -> anyhow::Result<Option<SpamReview>> {
    claim_review_delivery(pool, None, None).await
}

pub async fn claim_next_review_delivery_with_config(
    pool: &PgPool,
    config: &crate::config::Config,
) -> anyhow::Result<Option<SpamReview>> {
    claim_review_delivery(pool, None, Some(config)).await
}

pub async fn process_next_review_delivery(
    bot: &Bot,
    pool: &PgPool,
    config: &crate::config::Config,
) -> anyhow::Result<bool> {
    if !review_delivery_enabled(&config.community.moderation) {
        return suppress_pending_review_deliveries(pool).await;
    }

    let Some(review) = claim_next_review_delivery_with_config(pool, config).await? else {
        return Ok(false);
    };
    send_review(bot, pool, &review).await?;
    Ok(true)
}

pub fn review_delivery_enabled(moderation: &ModerationConfig) -> bool {
    moderation.enabled && moderation.review_delivery_enabled
}

pub async fn suppress_pending_review_deliveries(pool: &PgPool) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"
        update spam_review_requests
        set notification_status = 'failed',
            notification_error_kind = 'delivery_disabled',
            notification_processing_started_at = null,
            notification_lease_expires_at = null,
            notification_delivery_risk_score = null,
            notification_delivery_risk_signals = null,
            notification_delivery_review_threshold = null
        where status = 'pending'
          and notification_message_id is null
          and notification_status in ('pending', 'retry_wait', 'processing')
        "#,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

async fn claim_review_delivery(
    pool: &PgPool,
    request_id: Option<i64>,
    config: Option<&crate::config::Config>,
) -> anyhow::Result<Option<SpamReview>> {
    let row = sqlx::query(
        r#"
        with candidate as (
            select id
            from spam_review_requests
            where status = 'pending'
              and risk_score >= review_threshold
              and ($1::bigint is null or id = $1)
              and (
                  (notification_status in ('pending', 'retry_wait') and notification_next_attempt_at <= now())
                  or (notification_status = 'processing' and notification_lease_expires_at <= now())
              )
              and (
                  not exists (
                      select 1 from telegram_new_user_profile_audits audit
                      where audit.chat_id = spam_review_requests.chat_id
                        and audit.telegram_user_id = spam_review_requests.telegram_user_id
                        and audit.first_message_id is not null
                  )
                  or exists (
                      select 1
                      from telegram_new_user_profile_audits audit
                      join telegram_messages first_message
                        on first_message.chat_id = audit.chat_id
                       and first_message.message_id = audit.first_message_id
                      where audit.chat_id = spam_review_requests.chat_id
                        and audit.telegram_user_id = spam_review_requests.telegram_user_id
                        and first_message.user_id = audit.telegram_user_id
                        and first_message.source_channel_id is null
                        and first_message.created_at >= now() - ($3 * interval '1 second')
                  )
              )
            order by notification_next_attempt_at, id
            for update skip locked
            limit 1
        )
        update spam_review_requests request
        set notification_status = 'processing',
            notification_attempts = request.notification_attempts + 1,
            notification_lease_reclaim_count = request.notification_lease_reclaim_count
                + case when request.notification_status = 'processing' then 1 else 0 end,
            notification_processing_started_at = now(),
            notification_lease_expires_at = now() + ($2 * interval '1 second'),
            notification_delivery_risk_score = request.risk_score,
            notification_delivery_risk_signals = request.risk_signals,
            notification_delivery_review_threshold = request.review_threshold,
            notification_error_kind = null
        from candidate
        where request.id = candidate.id
        returning request.id, request.chat_id, request.telegram_user_id,
                  request.risk_score, request.risk_signals, request.notification_message_id,
                  request.notification_attempts, request.notification_consecutive_failures,
                  request.review_threshold
        "#,
    )
    .bind(request_id)
    .bind(DELIVERY_LEASE_SECONDS)
    .bind(FIRST_MESSAGE_REVIEW_MAX_AGE_SECONDS)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let id: i64 = row.get("id");
    let attempts: i32 = row.get("notification_attempts");
    let consecutive_failures: i32 = row.get("notification_consecutive_failures");
    match review_from_row(pool, row, config).await {
        Ok(review) => Ok(Some(review)),
        Err(error) => {
            mark_review_payload_build_failed(pool, id, attempts, consecutive_failures).await?;
            Err(error)
        }
    }
}

async fn review_from_row(
    pool: &PgPool,
    row: sqlx::postgres::PgRow,
    config: Option<&crate::config::Config>,
) -> anyhow::Result<SpamReview> {
    let id: i64 = row.get("id");
    let chat_id: i64 = row.get("chat_id");
    let user_id: i64 = row.get("telegram_user_id");
    let score: i32 = row.get("risk_score");
    let signals: Value = row.get("risk_signals");
    let notification_message_id: Option<i32> = row.get("notification_message_id");
    let notification_attempts: i32 = row.get("notification_attempts");
    let notification_consecutive_failures: i32 = row.get("notification_consecutive_failures");
    let stored_review_threshold: i32 = row.get("review_threshold");
    let profile = sqlx::query(r#"
        select cu.first_message_id,
               coalesce(nullif(trim(concat_ws(' ', p.first_name, p.last_name)), ''), 'Без имени') as name,
               p.username
        from (select $1::bigint as chat_id, $2::bigint as telegram_user_id) target
        left join telegram_chat_users cu
          on cu.chat_id = target.chat_id and cu.telegram_user_id = target.telegram_user_id
        left join telegram_user_profiles p on p.telegram_user_id = target.telegram_user_id
    "#)
    .bind(chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    let name: String = profile.get("name");
    let username: Option<String> = profile.get("username");
    let destination_chat_id = config
        .and_then(|config| config.community.moderation.review_chat.as_deref())
        .and_then(|key| config.and_then(|config| config.chat_by_key(key)))
        .map_or(chat_id, |chat| chat.config.id);
    let reasons = human_signals(&signals);
    let profile_url = format!("tg://user?id={user_id}");
    let profile_link = html::link(&name, &profile_url).into_string();
    let id_link = html::link(format!("id={user_id}"), &profile_url).into_string();
    let username = username
        .filter(|value| is_valid_telegram_username(value))
        .map(|value| html::link(format!("@{value}"), format!("https://t.me/{value}")).into_string())
        .unwrap_or_else(|| "без username".into());
    let text = format!(
        "<b>Проверка нового участника</b>\n\n{}\n{} · {} · риск: <b>{}</b>\n\n<b>Сигналы:</b>\n{}",
        profile_link, username, id_link, score, reasons
    );
    Ok(SpamReview {
        id,
        chat_id,
        destination_chat_id,
        first_message_id: profile.get("first_message_id"),
        notification_message_id,
        notification_attempts,
        notification_consecutive_failures,
        risk_score: score,
        review_threshold: stored_review_threshold,
        risk_signals: signals,
        text,
    })
}

async fn mark_review_payload_build_failed(
    pool: &PgPool,
    request_id: i64,
    attempts: i32,
    consecutive_failures: i32,
) -> anyhow::Result<()> {
    let (status, delay_seconds, error_kind) =
        match ANALYSIS_RETRY.delay_seconds(consecutive_failures.saturating_add(1), None) {
            Some(delay_seconds) => ("retry_wait", delay_seconds, "review_payload_build_failed"),
            None => ("failed", 0, "review_payload_build_retry_exhausted"),
        };
    sqlx::query(
        r#"
        update spam_review_requests
        set notification_status = $3,
            notification_next_attempt_at = now() + ($4 * interval '1 second'),
            notification_processing_started_at = null,
            notification_lease_expires_at = null,
            notification_error_kind = $5,
            notification_consecutive_failures = notification_consecutive_failures + 1
        where id = $1
          and notification_attempts = $2
          and status = 'pending'
          and notification_status = 'processing'
        "#,
    )
    .bind(request_id)
    .bind(attempts)
    .bind(status)
    .bind(delay_seconds)
    .bind(error_kind)
    .execute(pool)
    .await?;
    Ok(())
}

fn is_valid_telegram_username(value: &str) -> bool {
    let len = value.chars().count();
    (5..=32).contains(&len)
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

pub async fn send_review(bot: &Bot, pool: &PgPool, review: &SpamReview) -> anyhow::Result<()> {
    if confirm_review_delivery_payload(pool, review).await? == CasResult::LeaseLost {
        release_stale_review_delivery(pool, review).await?;
        tracing::warn!(
            request_id = review.id,
            attempts = review.notification_attempts,
            "spam review payload changed or delivery claim is no longer current; skipping Telegram delivery"
        );
        return Ok(());
    }

    let result = if let Some(message_id) = review.notification_message_id {
        bot.edit_message_text(
            ChatId(review.destination_chat_id),
            MessageId(message_id),
            &review.text,
        )
        .parse_mode(ParseMode::Html)
        .link_preview_options(disabled_link_preview())
        .reply_markup(review_keyboard(review.id))
        .await
        .map(|_| message_id)
    } else {
        let mut request = bot
            .send_message(ChatId(review.destination_chat_id), &review.text)
            .parse_mode(ParseMode::Html)
            .link_preview_options(disabled_link_preview())
            .reply_markup(review_keyboard(review.id));
        if review.destination_chat_id == review.chat_id
            && let Some(message_id) = review.first_message_id
        {
            request = request.reply_parameters(
                ReplyParameters::new(MessageId(message_id)).allow_sending_without_reply(),
            );
        }
        request.await.map(|message| message.id.0)
    };

    match result {
        Ok(message_id) => match mark_review_delivery_succeeded(pool, review, message_id).await? {
            CasResult::Applied => Ok(()),
            CasResult::LeaseLost => {
                tracing::warn!(
                    request_id = review.id,
                    attempts = review.notification_attempts,
                    "spam review delivery completion lost its lease"
                );
                Ok(())
            }
        },
        Err(err) => match classify_delivery_error(&err) {
            DeliveryFailure::AlreadyApplied => {
                match mark_review_delivery_succeeded(
                    pool,
                    review,
                    review.notification_message_id.unwrap_or_default(),
                )
                .await?
                {
                    CasResult::Applied => Ok(()),
                    CasResult::LeaseLost => {
                        tracing::warn!(
                            request_id = review.id,
                            attempts = review.notification_attempts,
                            "stale spam review edit completion lost its lease"
                        );
                        Ok(())
                    }
                }
            }
            failure => {
                let saved = mark_review_delivery_failed(pool, review, failure).await?;
                if saved == CasResult::LeaseLost {
                    tracing::warn!(
                        request_id = review.id,
                        attempts = review.notification_attempts,
                        "spam review delivery failure lost its lease"
                    );
                }
                Err(err.into())
            }
        },
    }
}

fn disabled_link_preview() -> LinkPreviewOptions {
    LinkPreviewOptions {
        is_disabled: true,
        url: None,
        prefer_small_media: false,
        prefer_large_media: false,
        show_above_text: false,
    }
}

async fn confirm_review_delivery_payload(
    pool: &PgPool,
    review: &SpamReview,
) -> anyhow::Result<CasResult> {
    let rows = sqlx::query(
        r#"
        update spam_review_requests
        set notification_lease_expires_at = now() + ($5 * interval '1 second')
        where id = $1
          and notification_attempts = $2
          and status = 'pending'
          and notification_status = 'processing'
          and notification_lease_expires_at > now()
          and review_threshold = $6
          and risk_score >= review_threshold
          and (risk_score, risk_signals) is not distinct from ($3, $4::jsonb)
          and (
              notification_delivery_risk_score,
              notification_delivery_risk_signals,
              notification_delivery_review_threshold
          ) is not distinct from ($3, $4::jsonb, $6)
        "#,
    )
    .bind(review.id)
    .bind(review.notification_attempts)
    .bind(review.risk_score)
    .bind(&review.risk_signals)
    .bind(DELIVERY_LEASE_SECONDS)
    .bind(review.review_threshold)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(rows.rows_affected())
}

async fn release_stale_review_delivery(pool: &PgPool, review: &SpamReview) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update spam_review_requests
        set notification_status = case
                when risk_score >= review_threshold then 'retry_wait'
                else 'pending'
            end,
            notification_next_attempt_at = now(),
            notification_processing_started_at = null,
            notification_lease_expires_at = null,
            notification_error_kind = null
        where id = $1
          and notification_attempts = $2
          and status = 'pending'
          and notification_status = 'processing'
          and (
              notification_delivery_risk_score,
              notification_delivery_risk_signals,
              notification_delivery_review_threshold
          ) is not distinct from ($3, $4::jsonb, $5)
          and (
              (risk_score, risk_signals) is distinct from ($3, $4::jsonb)
              or review_threshold is distinct from $5
          )
        "#,
    )
    .bind(review.id)
    .bind(review.notification_attempts)
    .bind(review.risk_score)
    .bind(&review.risk_signals)
    .bind(review.review_threshold)
    .execute(pool)
    .await?;
    Ok(())
}

fn review_keyboard(request_id: i64) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new([[
        InlineKeyboardButton::callback("Верно: спамер", format!("spam_review:{request_id}:spam")),
        InlineKeyboardButton::callback(
            "Неверно: не спамер",
            format!("spam_review:{request_id}:normal"),
        ),
    ]])
}

pub async fn mark_review_delivery_succeeded(
    pool: &PgPool,
    review: &SpamReview,
    message_id: i32,
) -> anyhow::Result<CasResult> {
    let rows = sqlx::query(
        r#"
        update spam_review_requests
        set notification_status = case
                when (risk_score, risk_signals) is distinct from ($3, $4::jsonb)
                    then 'retry_wait'
                else 'sent'
            end,
            notified_at = now(), notification_message_id = $2,
            notified_risk_score = $3, notified_risk_signals = $4,
            notification_next_attempt_at = case
                when (risk_score, risk_signals) is distinct from ($3, $4::jsonb) then now()
                else notification_next_attempt_at
            end,
            notification_processing_started_at = null,
            notification_lease_expires_at = null,
            notification_error_kind = null,
            notification_consecutive_failures = 0
        where id = $1
          and notification_attempts = $5
          and status = 'pending'
          and notification_status = 'processing'
          and (
              notification_delivery_risk_score,
              notification_delivery_risk_signals,
              notification_delivery_review_threshold
          ) is not distinct from ($3, $4::jsonb, $6)
        "#,
    )
    .bind(review.id)
    .bind(message_id)
    .bind(review.risk_score)
    .bind(&review.risk_signals)
    .bind(review.notification_attempts)
    .bind(review.review_threshold)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(rows.rows_affected())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeliveryFailure {
    Retryable { retry_after_seconds: Option<i64> },
    ReplaceMessage,
    Terminal(&'static str),
    AlreadyApplied,
}

fn classify_delivery_error(error: &teloxide::RequestError) -> DeliveryFailure {
    if let teloxide::RequestError::RetryAfter(seconds) = error {
        return DeliveryFailure::Retryable {
            retry_after_seconds: Some(i64::from(seconds.seconds())),
        };
    }
    if matches!(
        error,
        teloxide::RequestError::Api(teloxide::ApiError::InvalidToken)
    ) {
        return DeliveryFailure::Terminal("telegram_invalid_token");
    }

    let message = error.to_string().to_lowercase();
    if message.contains("message is not modified") {
        DeliveryFailure::AlreadyApplied
    } else if message.contains("message to edit not found")
        || message.contains("message can't be edited")
    {
        DeliveryFailure::ReplaceMessage
    } else if message.contains("forbidden") || message.contains("chat not found") {
        DeliveryFailure::Terminal("telegram_forbidden")
    } else {
        DeliveryFailure::Retryable {
            retry_after_seconds: None,
        }
    }
}

async fn mark_review_delivery_failed(
    pool: &PgPool,
    review: &SpamReview,
    failure: DeliveryFailure,
) -> anyhow::Result<CasResult> {
    let (status, error_kind, clear_message_id, delay_seconds, increment_failures) = match failure {
        DeliveryFailure::Retryable {
            retry_after_seconds,
        } => match ANALYSIS_RETRY.delay_seconds(
            review.notification_consecutive_failures + 1,
            retry_after_seconds,
        ) {
            Some(delay) => (
                "retry_wait",
                "telegram_send_failed",
                false,
                Some(delay),
                true,
            ),
            None => ("failed", "telegram_retry_exhausted", false, None, true),
        },
        DeliveryFailure::ReplaceMessage => (
            "retry_wait",
            "telegram_message_missing",
            true,
            Some(0),
            false,
        ),
        DeliveryFailure::Terminal(kind) => ("failed", kind, false, None, false),
        DeliveryFailure::AlreadyApplied => {
            anyhow::bail!("already-applied delivery must be finalized as success")
        }
    };
    let rows = sqlx::query(
        r#"
        update spam_review_requests
        set notification_status = $2,
            notification_next_attempt_at = now() + (coalesce($3, 0) * interval '1 second'),
            notification_message_id = case when $4 then null else notification_message_id end,
            notification_processing_started_at = null,
            notification_lease_expires_at = null,
            notification_error_kind = $5,
            notification_consecutive_failures = notification_consecutive_failures + case when $6 then 1 else 0 end
        where id = $1
          and notification_attempts = $7
          and status = 'pending'
          and notification_status = 'processing'
          and (risk_score, risk_signals) is not distinct from ($8, $9::jsonb)
          and (
              notification_delivery_risk_score,
              notification_delivery_risk_signals,
              notification_delivery_review_threshold
          ) is not distinct from ($8, $9::jsonb, $10)
        "#,
    )
    .bind(review.id)
    .bind(status)
    .bind(delay_seconds)
    .bind(clear_message_id)
    .bind(error_kind)
    .bind(increment_failures)
    .bind(review.notification_attempts)
    .bind(review.risk_score)
    .bind(&review.risk_signals)
    .bind(review.review_threshold)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(rows.rows_affected())
}

pub async fn apply_callback(
    pool: &PgPool,
    request_id: i64,
    decision: &str,
    owner_id: i64,
) -> anyhow::Result<Option<&'static str>> {
    let status = match decision {
        "spam" => "confirmed_spam",
        "normal" => "confirmed_not_spam",
        _ => return Ok(None),
    };
    let mut tx = pool.begin().await?;
    let row = sqlx::query("update spam_review_requests set status = $2, reviewed_at = now(), reviewed_by_user_id = $3 where id = $1 and status = 'pending' returning chat_id, telegram_user_id, risk_signals")
        .bind(request_id).bind(status).bind(owner_id).fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    if decision == "spam" {
        let chat_id: i64 = row.get("chat_id");
        let user_id: i64 = row.get("telegram_user_id");
        let risk_signals: Value = row.get("risk_signals");
        record_spam_in_transaction(
            &mut tx,
            chat_id,
            user_id,
            &SpamLabel {
                subtype: owner_review_spam_subtype(&risk_signals).to_string(),
                source: LabelSource::OwnerReview,
                reason: "Owner-confirmed spammer".to_string(),
                evidence: serde_json::json!({"review_id": request_id}),
                operator_id: Some(owner_id),
            },
        )
        .await?;
    } else if decision == "normal" {
        let chat_id: i64 = row.get("chat_id");
        let user_id: i64 = row.get("telegram_user_id");
        record_not_spam_in_transaction(
            &mut tx,
            chat_id,
            user_id,
            "Owner rejected spam review",
            &serde_json::json!({"review_id": request_id}),
            Some(owner_id),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Some(if decision == "spam" {
        "Помечено как спамер."
    } else {
        "Помечено как не спамер."
    }))
}

/// Derives the corpus subtype for an owner-confirmed spammer from the stored
/// risk signals. Falls back to a generic comment subtype when no known
/// campaign marker is present.
fn owner_review_spam_subtype(signals: &Value) -> &'static str {
    let labels = signals
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|signal| signal.get("label").and_then(Value::as_str))
        .collect::<Vec<_>>();

    if labels.iter().any(|label| {
        matches!(
            *label,
            "explicit_adult_promo_bio" | "personal_channel_adult_links"
        )
    }) {
        return "adult_personal_channel_promo";
    }
    if labels.iter().any(|label| {
        matches!(
            *label,
            "foreign_invite_link_message" | "invite_link_from_new_user"
        )
    }) {
        return "foreign_invite_link_spam";
    }
    if labels.iter().any(|label| {
        matches!(
            *label,
            "profile_bio_subscription_invite_offer"
                | "personal_channel_invite_link"
                | "personal_channel_external_link"
        )
    }) {
        return "profile_channel_bait";
    }

    let has_unified_campaign = signals
        .as_array()
        .into_iter()
        .flatten()
        .filter(|signal| {
            signal.get("label").and_then(Value::as_str) == Some("unified_first_message_analysis")
        })
        .any(|signal| {
            signal
                .get("assessment")
                .and_then(|assessment| assessment.get("template_campaign"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || signal
                    .get("assessment")
                    .and_then(|assessment| assessment.get("direct_dm_offer"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        });
    if has_unified_campaign {
        return "promo_dm_bait";
    }

    "llm_generic_comment"
}

pub fn parse_callback(data: &str) -> Option<(i64, &str)> {
    let mut parts = data.split(':');
    (parts.next()? == "spam_review").then_some(())?;
    let id = parts.next()?.parse().ok()?;
    let decision = parts.next()?;
    parts.next().is_none().then_some((id, decision))
}

pub fn callback_message_chat_id(query: &teloxide::types::CallbackQuery) -> Option<i64> {
    query.message.as_ref().map(|message| message.chat().id.0)
}

pub async fn is_chat_admin(
    bot: &Bot,
    chat_id: i64,
    user_id: i64,
) -> Result<bool, teloxide::RequestError> {
    if user_id <= 0 {
        return Ok(false);
    }
    let member = bot
        .get_chat_member(ChatId(chat_id), teloxide::types::UserId(user_id as u64))
        .await?;
    Ok(matches!(
        member.kind,
        teloxide::types::ChatMemberKind::Administrator(_)
            | teloxide::types::ChatMemberKind::Owner(_)
    ))
}

fn human_signals(signals: &Value) -> String {
    let labels = signals
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|signal| {
            let mut labels = signal
                .get("label")
                .and_then(Value::as_str)
                .map(human_label)
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>();
            if signal.get("label").and_then(Value::as_str) == Some("unified_first_message_analysis")
            {
                labels.extend(
                    signal["assessment"]["risk_markers"]
                        .as_array()
                        .or_else(|| signal["assessment"]["markers"].as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(human_marker),
                );
            }
            if signal.get("label").and_then(Value::as_str) == Some("first_message_text_observation")
            {
                labels.extend(human_text_observations(signal));
            }
            labels
        })
        .collect::<Vec<_>>();
    if labels.is_empty() {
        "—".to_string()
    } else {
        labels
            .into_iter()
            .map(|label| format!("• {}", html::escape(&label)))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn human_text_observations(signal: &Value) -> Vec<String> {
    let mut labels = Vec::new();
    let flags = &signal["text_observations"];
    if flags["nfkc_changed"].as_bool() == Some(true) {
        labels.push("Unicode: совместимые формы/шрифты нормализованы".to_owned());
    }
    for (key, name) in [
        ("mixed_script_words", "слова со смешанными алфавитами"),
        ("homoglyph_chars", "замены похожих букв"),
        ("removed_invisible_chars", "невидимые символы внутри текста"),
        ("bidi_controls", "управление направлением текста"),
        ("removed_word_variation_selectors", "селекторы внутри слов"),
        ("removed_stacked_marks", "стэки диакритики"),
        ("spaced_letter_sequences", "растянутые слова"),
        ("long_repeated_letter_runs", "длинные повторы букв"),
    ] {
        if let Some(count) = flags[key].as_u64().filter(|&count| count > 0) {
            labels.push(format!("Unicode: {name}: {count}"));
        }
    }
    if let Some(probability) = signal["linear_spam_probability"].as_f64() {
        labels.push(format!("Локальная модель: p={probability:.3}"));
    }
    if let Some(probability) = signal["embedding_spam_probability"].as_f64() {
        labels.push(format!("Эмбеддинг-модель: p={probability:.3}"));
    }
    if let Some(probability) = signal["reputation_probability"].as_f64() {
        labels.push(format!("Репутация: p={probability:.3}"));
    }
    if let Some(top) = signal["category_scores"]["scores"]
        .as_array()
        .and_then(|scores| {
            scores
                .iter()
                .filter_map(|score| {
                    let probability = score["probability"].as_f64()?;
                    let category = score["category"].as_str()?;
                    (probability.is_finite()
                        && (0.0..=1.0).contains(&probability)
                        && probability >= 0.5)
                        .then_some((category, probability))
                })
                .max_by(|left, right| left.1.total_cmp(&right.1).then_with(|| left.0.cmp(right.0)))
        })
    {
        labels.push(format!("Категория: {} (p={:.2})", top.0, top.1));
    }
    if let Some(suspected) = signal["suspected_categories"].as_array() {
        let names: Vec<&str> = suspected.iter().filter_map(Value::as_str).collect();
        if !names.is_empty() {
            labels.push(format!("Маркеры указывают на: {}", names.join(", ")));
        }
    }
    labels
}

fn human_marker(marker: &str) -> String {
    match marker {
        "paid_easy_task_offer" => "LLM: обещание лёгкой оплачиваемой работы".to_string(),
        "external_promo_funnel" => "LLM: promo-воронка или внешний увод".to_string(),
        "send_or_share_offer" => "LLM: предложение прислать материал".to_string(),
        "direct_messages" => "LLM: перевод разговора в личные сообщения".to_string(),
        "template_efficiency_narrative" => "LLM: шаблонный мотивирующий нарратив".to_string(),
        "self_help_or_finance_promo" => "LLM: оффтопное self-help или финансовое promo".to_string(),
        "masked_call_to_action" => "LLM: замаскированный призыв к действию".to_string(),
        "generic_campaign_reaction" => {
            "LLM: шаблонная реакция без самостоятельного штрафа".to_string()
        }
        "performative_feminine_persona" => "LLM: нарочито женственный шаблонный образ".to_string(),
        "rkn_related_vpn_promotion" => "LLM: VPN-промо в теме блокировок/РКН".to_string(),
        other => format!("LLM: {other}"),
    }
}

fn human_label(label: &str) -> &str {
    match label {
        "shared_spammer_identity" => "ID уже помечен спамером в другом инстансе",
        "lols_spammer_identity" => "ID есть в LOLS banlist спамеров",
        "recent_high_telegram_id" => "очень свежий Telegram ID",
        "telegram_id_spam_probability" => "свежий Telegram ID по модели",
        "single_message_account" => "первое и единственное сообщение",
        "very_new_to_chat" => "недавно появился в чате",
        "only_channel_post_comments" => "комментирует только посты канала",
        "reply_to_channel_post_not_comment" => "ответил прямо на пост, не на обсуждение",
        "display_name_reused_by_spammers" => "имя уже встречалось у размеченных спамеров",
        "personal_channel_title_reused_by_spammers" => {
            "название личного канала уже встречалось у размеченных спамеров"
        }
        "identity_display_name_rotation" => "пользователь менял отображаемое имя",
        "identity_username_rotation" => "пользователь менял username",
        "display_name_reused_by_mixed_labels" => {
            "имя встречалось и у спамеров, и у подтверждённых нормальных пользователей"
        }
        "display_name_reused_by_confirmed_normal" => {
            "имя встречалось только у подтверждённых нормальных пользователей"
        }
        "username_reused_by_spammers" => "username уже встречался у размеченных спамеров",
        "username_reused_by_mixed_labels" => {
            "username встречался и у спамеров, и у подтверждённых нормальных пользователей"
        }
        "username_reused_by_confirmed_normal" => {
            "username встречался только у подтверждённых нормальных пользователей"
        }
        "username_random_suffix" => "username похож на автоматически созданный",
        "mixed_script_profile_homoglyphs" => {
            "в имени смешаны похожие латинские и кириллические буквы"
        }
        "first_message_text_observation" => "Разбор текста: наблюдения без самостоятельного штрафа",
        "explicit_adult_promo_bio" => "bio рекламирует adult-сервис через ссылку или воронку",
        "personal_channel_attached" => "подключён личный канал",
        "llm_personal_channel_content_promotion" => {
            "LLM: рекламная воронка в содержимом личного канала"
        }
        "personal_channel_external_link" => "в личном канале есть внешняя ссылка",
        "non_adjacent_emoji_message" => "нетипичный emoji в комментарии",
        "non_adjacent_emoji_message_ending" => "комментарий заканчивается emoji",
        "unified_first_message_analysis" => "первое сообщение похоже на известную спам-кампанию",
        "rkn_vpn_service_promotion" => "промо VPN в ответ на вопрос об ограничениях/РКН",
        "offtopic_direct_dm_funnel" => "оффтопное предложение перейти в личку и прислать материал",
        "offtopic_external_promo_funnel" => "оффтопная реклама стороннего сервиса или бота",
        "evidence_backed_paid_task_offer" => {
            "подтверждённое предложением сообщение о лёгком заработке"
        }
        "known_spam_campaign_match" => "первое сообщение совпадает с известной спам-кампанией",
        "tree_personal_channel_adult_funnel" => "личный канал ведёт в adult-воронку",
        "tree_personal_channel_invite_funnel" => "личный канал ведёт по Telegram-инвайту",
        "tree_fresh_money_work_contact_funnel" => {
            "свежий участник предлагает заработок/работу с прямым CTA"
        }
        "tree_fresh_contact_send_offer" => "свежий участник по CTA обещает прислать материал",
        "tree_fresh_paid_task_offer" => {
            "свежий участник обещает конкретную оплату за простую задачу"
        }
        "tree_fresh_recent_id_repeated_message" => {
            "свежий ID повторяет сообщение в короткой кампании"
        }
        "tree_fresh_channel_external_link" => "свежий участник ведёт во внешний канал/ссылку",
        "tree_channel_comments_with_recent_id" => "свежий ID пишет только ответы к постам канала",
        "tree_channel_comments_with_personal_channel" => {
            "ответы только к постам канала и привязанный личный канал"
        }
        "tree_personal_channel_random_username_single_message" => {
            "одно сообщение, случайный username и подключённый личный канал"
        }
        "tree_recent_id_random_username" => "свежий ID и username со случайным суффиксом",
        _ => label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    #[test]
    fn review_delivery_can_be_paused_without_disabling_moderation() {
        let paused: ModerationConfig = toml::from_str(
            "enabled = true\nreview_delivery_enabled = false\nrisk_profile = 'ru_general_v1'",
        )
        .unwrap();
        assert!(!review_delivery_enabled(&paused));

        let legacy: ModerationConfig =
            toml::from_str("enabled = true\nrisk_profile = 'ru_general_v1'").unwrap();
        assert!(review_delivery_enabled(&legacy));
    }

    #[test]
    fn parses_callback() {
        assert_eq!(parse_callback("spam_review:42:spam"), Some((42, "spam")));
        assert_eq!(parse_callback("spam_review:42:spam:x"), None);
    }

    #[tokio::test]
    async fn review_chat_admins_are_authorized_but_regular_members_are_not() {
        assert!(mocked_chat_member_is_admin("administrator").await);
        assert!(!mocked_chat_member_is_admin("member").await);
        assert!(
            !is_chat_admin(&Bot::new("test-token"), -1001, 0)
                .await
                .unwrap()
        );
    }

    #[test]
    fn callback_from_inaccessible_old_card_keeps_review_chat_context() {
        let query: teloxide::types::CallbackQuery = serde_json::from_value(serde_json::json!({
            "id": "callback-id",
            "from": {"id": 42, "is_bot": false, "first_name": "Admin"},
            "message": {
                "message_id": 7,
                "date": 0,
                "chat": {"id": -100123, "type": "supergroup", "title": "Review"}
            },
            "chat_instance": "review-chat-instance",
            "data": "spam_review:42:spam"
        }))
        .expect("callback update with an old inaccessible message should deserialize");

        assert!(query.regular_message().is_none());
        assert_eq!(callback_message_chat_id(&query), Some(-100123));
    }

    async fn mocked_chat_member_is_admin(status: &'static str) -> bool {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let response_task = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 2_048];
            let _ = stream.read(&mut request);
            let member = match status {
                "administrator" => serde_json::json!({
                    "user": {"id": 42, "is_bot": false, "first_name": "Admin"},
                    "status": "administrator",
                    "can_be_edited": false,
                    "is_anonymous": false,
                    "can_manage_chat": true,
                    "can_change_info": false,
                    "can_delete_messages": false,
                    "can_manage_video_chats": false,
                    "can_invite_users": false,
                    "can_restrict_members": false,
                    "can_promote_members": false
                }),
                "member" => serde_json::json!({
                    "user": {"id": 42, "is_bot": false, "first_name": "Member"},
                    "status": "member"
                }),
                _ => unreachable!(),
            };
            let body = serde_json::json!({"ok": true, "result": member}).to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let bot = Bot::new("test-token").set_api_url(
            format!("http://{address}/")
                .parse()
                .expect("mock Telegram API URL must parse"),
        );
        let result = is_chat_admin(&bot, -100123, 42).await.unwrap();
        response_task.join().unwrap();
        result
    }

    #[test]
    fn keeps_telegram_retry_after_for_delivery_delay() {
        let failure = classify_delivery_error(&teloxide::RequestError::RetryAfter(
            teloxide::types::Seconds::from_seconds(75),
        ));
        assert_eq!(
            failure,
            DeliveryFailure::Retryable {
                retry_after_seconds: Some(75)
            }
        );
    }

    #[test]
    fn human_signals_renders_unified_first_message_markers() {
        let unified = serde_json::json!([{
            "label": "unified_first_message_analysis",
            "assessment": { "risk_markers": ["direct_messages"] }
        }]);
        let additional = serde_json::json!([{
            "label": "unified_first_message_analysis",
            "assessment": { "markers": ["paid_easy_task_offer"] }
        }]);

        assert!(human_signals(&unified).contains("перевод разговора в личные сообщения"));
        assert!(human_signals(&additional).contains("обещание лёгкой оплачиваемой работы"));
    }

    #[test]
    fn text_observations_render_facts_without_calling_them_spam() {
        let signals = serde_json::json!([{
            "label": "first_message_text_observation", "coefficient": 0,
            "linear_spam_probability": 0.2,
            "embedding_spam_probability": 0.85,
            "reputation_probability": 0.92,
            "category_scores": {"version": "test-v1", "scores": [
                {"category": "job_scam", "probability": 0.9},
                {"category": "vpn_promo", "probability": 0.1},
            ]},
            "suspected_categories": ["job_scam"],
            "text_observations": {"nfkc_changed": true, "homoglyph_chars": 3, "bidi_controls": 1},
        }]);
        let rendered = human_signals(&signals);
        assert!(rendered.contains("без самостоятельного штрафа"));
        assert!(rendered.contains("замены похожих букв: 3"));
        assert!(rendered.contains("управление направлением текста: 1"));
        assert!(rendered.contains("p=0.200"));
        assert!(rendered.contains("Эмбеддинг-модель: p=0.850"));
        assert!(rendered.contains("Репутация: p=0.920"));
        assert!(rendered.contains("job_scam (p=0.90)"));
        assert!(rendered.contains("Маркеры указывают на: job_scam"));
        assert!(!rendered.contains("растянутые слова"));
    }

    #[test]
    fn renders_human_signal() {
        assert_eq!(
            human_label("shared_spammer_identity"),
            "ID уже помечен спамером в другом инстансе"
        );
        assert_eq!(
            human_label("recent_high_telegram_id"),
            "очень свежий Telegram ID"
        );
        assert_eq!(
            human_label("telegram_id_spam_probability"),
            "свежий Telegram ID по модели"
        );
        assert_eq!(
            human_label("tree_channel_comments_with_personal_channel"),
            "ответы только к постам канала и привязанный личный канал"
        );
        assert_eq!(
            human_label("offtopic_direct_dm_funnel"),
            "оффтопное предложение перейти в личку и прислать материал"
        );
        assert_eq!(
            human_label("offtopic_external_promo_funnel"),
            "оффтопная реклама стороннего сервиса или бота"
        );
    }
}
