use serde_json::Value;
use sqlx::{PgPool, Row};
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId, ParseMode, ReplyParameters},
};

use crate::{
    features::jobs::{claim::CasResult, policy::ANALYSIS_RETRY},
    telegram::html,
};

const OWNER_USERNAME: &str = "Chechulinm";
const DELIVERY_LEASE_SECONDS: i64 = 10 * 60;

pub struct SpamReview {
    pub id: i64,
    pub chat_id: i64,
    pub first_message_id: Option<i32>,
    pub notification_message_id: Option<i32>,
    pub notification_attempts: i32,
    pub notification_consecutive_failures: i32,
    pub risk_score: i32,
    pub risk_signals: Value,
    pub text: String,
}

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
    claim_review_delivery(pool, Some(request_id)).await
}

pub async fn claim_next_review_delivery(pool: &PgPool) -> anyhow::Result<Option<SpamReview>> {
    claim_review_delivery(pool, None).await
}

async fn claim_review_delivery(
    pool: &PgPool,
    request_id: Option<i64>,
) -> anyhow::Result<Option<SpamReview>> {
    let row = sqlx::query(
        r#"
        with candidate as (
            select id
            from spam_review_requests
            where status = 'pending'
              and risk_score >= 70
              and ($1::bigint is null or id = $1)
              and (
                  (notification_status in ('pending', 'retry_wait') and notification_next_attempt_at <= now())
                  or (notification_status = 'processing' and notification_lease_expires_at <= now())
              )
            order by notification_next_attempt_at, id
            for update skip locked
            limit 1
        )
        update spam_review_requests request
        set notification_status = 'processing',
            notification_attempts = request.notification_attempts + 1,
            notification_processing_started_at = now(),
            notification_lease_expires_at = now() + ($2 * interval '1 second'),
            notification_error_kind = null
        from candidate
        where request.id = candidate.id
        returning request.id, request.chat_id, request.telegram_user_id,
                  request.risk_score, request.risk_signals, request.notification_message_id,
                  request.notification_attempts, request.notification_consecutive_failures
        "#,
    )
    .bind(request_id)
    .bind(DELIVERY_LEASE_SECONDS)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    review_from_row(pool, row).await.map(Some)
}

async fn review_from_row(pool: &PgPool, row: sqlx::postgres::PgRow) -> anyhow::Result<SpamReview> {
    let id: i64 = row.get("id");
    let chat_id: i64 = row.get("chat_id");
    let user_id: i64 = row.get("telegram_user_id");
    let score: i32 = row.get("risk_score");
    let signals: Value = row.get("risk_signals");
    let notification_message_id: Option<i32> = row.get("notification_message_id");
    let notification_attempts: i32 = row.get("notification_attempts");
    let notification_consecutive_failures: i32 = row.get("notification_consecutive_failures");
    let profile = sqlx::query(r#"
        select cu.first_message_id, coalesce(nullif(trim(concat_ws(' ', p.first_name, p.last_name)), ''), 'Без имени') as name,
               p.username
        from telegram_chat_users cu left join telegram_user_profiles p on p.telegram_user_id = cu.telegram_user_id
        where cu.chat_id = $1 and cu.telegram_user_id = $2
    "#).bind(chat_id).bind(user_id).fetch_one(pool).await?;
    let name: String = profile.get("name");
    let username: Option<String> = profile.get("username");
    let reasons = human_signals(&signals);
    let profile_url = format!("tg://user?id={user_id}");
    let profile_link = html::link(&name, &profile_url).into_string();
    let id_link = html::link(format!("id={user_id}"), &profile_url).into_string();
    let username = username
        .filter(|value| is_valid_telegram_username(value))
        .map(|value| html::link(format!("@{value}"), format!("https://t.me/{value}")).into_string())
        .unwrap_or_else(|| "без username".into());
    let text = format!(
        "@{OWNER_USERNAME}, <b>проверка нового участника</b>\n\n{}\n{} · {} · риск: <b>{}</b>\n\n<b>Сигналы:</b>\n{}",
        profile_link, username, id_link, score, reasons
    );
    Ok(SpamReview {
        id,
        chat_id,
        first_message_id: profile.get("first_message_id"),
        notification_message_id,
        notification_attempts,
        notification_consecutive_failures,
        risk_score: score,
        risk_signals: signals,
        text,
    })
}

fn is_valid_telegram_username(value: &str) -> bool {
    let len = value.chars().count();
    (5..=32).contains(&len)
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

pub async fn send_review(bot: &Bot, pool: &PgPool, review: &SpamReview) -> anyhow::Result<()> {
    let result = if let Some(message_id) = review.notification_message_id {
        bot.edit_message_text(ChatId(review.chat_id), MessageId(message_id), &review.text)
            .parse_mode(ParseMode::Html)
            .reply_markup(review_keyboard(review.id))
            .await
            .map(|_| message_id)
    } else {
        let mut request = bot
            .send_message(ChatId(review.chat_id), &review.text)
            .parse_mode(ParseMode::Html)
            .reply_markup(review_keyboard(review.id));
        if let Some(message_id) = review.first_message_id {
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
        "#,
    )
    .bind(review.id)
    .bind(message_id)
    .bind(review.risk_score)
    .bind(&review.risk_signals)
    .bind(review.notification_attempts)
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
        "#,
    )
    .bind(review.id)
    .bind(status)
    .bind(delay_seconds)
    .bind(clear_message_id)
    .bind(error_kind)
    .bind(increment_failures)
    .bind(review.notification_attempts)
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
    let row = sqlx::query("update spam_review_requests set status = $2, reviewed_at = now(), reviewed_by_user_id = $3 where id = $1 and status = 'pending' returning chat_id, telegram_user_id")
        .bind(request_id).bind(status).bind(owner_id).fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    if decision == "spam" {
        let chat_id: i64 = row.get("chat_id");
        let user_id: i64 = row.get("telegram_user_id");
        sqlx::query("update telegram_chat_users set is_spammer = true, spam_score = greatest(spam_score, 100), spam_last_marked_at = now(), spam_reason = 'Owner-confirmed spammer', spam_type = 'llm_generic_comment', spam_types = jsonb_set(coalesce(spam_types, '{}'::jsonb), '{llm_generic_comment}', '1'::jsonb, true), updated_at = now() where chat_id = $1 and telegram_user_id = $2").bind(chat_id).bind(user_id).execute(&mut *tx).await?;
        sqlx::query("update telegram_messages set spam_marked_at = coalesce(spam_marked_at, now()), spam_reason = 'Owner-confirmed spammer', spam_source = 'manual_owner_confirmation', spam_type = coalesce(spam_type, 'llm_generic_comment') where chat_id = $1 and user_id = $2 and source_channel_id is null").bind(chat_id).bind(user_id).execute(&mut *tx).await?;
        sqlx::query("update telegram_chat_users set spam_message_count = (select count(*) from telegram_messages where chat_id = $1 and user_id = $2 and spam_marked_at is not null), spam_types = jsonb_set(coalesce(spam_types, '{}'::jsonb), '{llm_generic_comment}', to_jsonb((select count(*) from telegram_messages where chat_id = $1 and user_id = $2 and spam_marked_at is not null)), true) where chat_id = $1 and telegram_user_id = $2").bind(chat_id).bind(user_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Some(if decision == "spam" {
        "Помечено как спамер."
    } else {
        "Помечено как не спамер."
    }))
}

pub fn parse_callback(data: &str) -> Option<(i64, &str)> {
    let mut parts = data.split(':');
    (parts.next()? == "spam_review").then_some(())?;
    let id = parts.next()?.parse().ok()?;
    let decision = parts.next()?;
    parts.next().is_none().then_some((id, decision))
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
            if signal.get("label").and_then(Value::as_str) == Some("first_message_spam_analysis") {
                labels.extend(
                    signal["assessment"]["markers"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(human_marker),
                );
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
        other => format!("LLM: {other}"),
    }
}

fn human_label(label: &str) -> &str {
    match label {
        "recent_high_telegram_id" => "очень свежий Telegram ID",
        "single_message_account" => "первое и единственное сообщение",
        "very_new_to_chat" => "недавно появился в чате",
        "only_channel_post_comments" => "комментирует только посты канала",
        "reply_to_channel_post_not_comment" => "ответил прямо на пост, не на обсуждение",
        "display_name_reused_by_spammers" => "имя уже встречалось у размеченных спамеров",
        "username_random_suffix" => "username похож на автоматически созданный",
        "mixed_script_profile_homoglyphs" => {
            "в имени смешаны похожие латинские и кириллические буквы"
        }
        "explicit_adult_promo_bio" => "bio рекламирует adult-сервис через ссылку или воронку",
        "personal_channel_attached" => "подключён личный канал",
        "personal_channel_external_link" => "в личном канале есть внешняя ссылка",
        "non_adjacent_emoji_message" => "нетипичный emoji в комментарии",
        "non_adjacent_emoji_message_ending" => "комментарий заканчивается emoji",
        "first_message_spam_analysis" => "первое сообщение похоже на известную спам-кампанию",
        _ => label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_callback() {
        assert_eq!(parse_callback("spam_review:42:spam"), Some((42, "spam")));
        assert_eq!(parse_callback("spam_review:42:spam:x"), None);
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
    fn renders_human_signal() {
        assert_eq!(
            human_label("recent_high_telegram_id"),
            "очень свежий Telegram ID"
        );
    }
}
