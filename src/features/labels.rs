use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};

/// Единая точка записи durable-разметки спамеров.
///
/// До этого модуля пометки размазывались: кнопки review-карточек обновляли
/// только флаги `telegram_chat_users` и штампы сообщений, а ручные SQL-батчи —
/// ещё и `spam_label_events`. Без событий в `spam_label_events` reuse-сигналы
/// (`display_name/username/channel_title_reused_by_spammers`) и template-корпус
/// слепнут: `is_spammer` на пользователе для них недостаточно.
/// Источник разметки. `OwnerManual` (ручные SQL-батчи) и `System`
/// (автомодерация) зарезервированы под следующие слайсы enforcement ladder.
pub enum LabelSource {
    OwnerReview,
    #[allow(dead_code)]
    OwnerManual,
    #[allow(dead_code)]
    System,
}

impl LabelSource {
    fn as_str(&self) -> &'static str {
        match self {
            Self::OwnerReview => "owner_review",
            Self::OwnerManual => "owner_manual",
            Self::System => "system",
        }
    }
}

pub struct SpamLabel {
    pub subtype: String,
    pub source: LabelSource,
    pub reason: String,
    pub evidence: Value,
    pub operator_id: Option<i64>,
}

/// Помечает пользователя спамером: событие + флаги + штампы сообщений +
/// пересчёт счётчика. Семантика повторяет прежний `apply_callback("spam")`,
/// плюс пишется `spam_label_events`.
// NOTE: прямой вызов появится в enforcement ladder; пока используется
// транзакционная версия из review-колбэка.
#[allow(dead_code)]
pub async fn record_spam(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
    label: &SpamLabel,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    record_spam_in_transaction(&mut tx, chat_id, user_id, label).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn record_spam_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    chat_id: i64,
    user_id: i64,
    label: &SpamLabel,
) -> anyhow::Result<()> {
    sqlx::query(
        "insert into spam_label_events (chat_id, telegram_user_id, label, subtype, source, reason, evidence, operator_telegram_user_id) values ($1, $2, 'spam', $3, $4, $5, $6, $7)",
    )
    .bind(chat_id)
    .bind(user_id)
    .bind(&label.subtype)
    .bind(label.source.as_str())
    .bind(&label.reason)
    .bind(&label.evidence)
    .bind(label.operator_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("update telegram_chat_users set is_spammer = true, spam_score = greatest(spam_score, 100), spam_last_marked_at = now(), spam_reason = $3, spam_type = $4, updated_at = now() where chat_id = $1 and telegram_user_id = $2")
        .bind(chat_id)
        .bind(user_id)
        .bind(&label.reason)
        .bind(&label.subtype)
        .execute(&mut **tx)
        .await?;
    sqlx::query("update telegram_messages set spam_marked_at = coalesce(spam_marked_at, now()), spam_reason = $3, spam_source = 'manual_owner_confirmation', spam_type = coalesce(spam_type, $4) where chat_id = $1 and user_id = $2 and source_channel_id is null")
        .bind(chat_id)
        .bind(user_id)
        .bind(&label.reason)
        .bind(&label.subtype)
        .execute(&mut **tx)
        .await?;
    sqlx::query("update telegram_chat_users set spam_message_count = (select count(*) from telegram_messages where chat_id = $1 and user_id = $2 and spam_marked_at is not null), spam_types = coalesce(spam_types, '{}'::jsonb) || jsonb_build_object($3, (select count(*) from telegram_messages where chat_id = $1 and user_id = $2 and spam_marked_at is not null)) where chat_id = $1 and telegram_user_id = $2")
        .bind(chat_id)
        .bind(user_id)
        .bind(&label.subtype)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Снимает пометку: событие `not_spam` + сброс флагов + снятие ручных штампов.
/// Семантика повторяет прежний `apply_callback("normal")`, плюс пишется
/// `spam_label_events`, откуда reuse-счётчики берут confirmed-normal.
pub async fn record_not_spam(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
    reason: &str,
    evidence: &Value,
    operator_id: Option<i64>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    record_not_spam_in_transaction(&mut tx, chat_id, user_id, reason, evidence, operator_id)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn record_not_spam_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    chat_id: i64,
    user_id: i64,
    reason: &str,
    evidence: &Value,
    operator_id: Option<i64>,
) -> anyhow::Result<()> {
    sqlx::query(
        "insert into spam_label_events (chat_id, telegram_user_id, label, subtype, source, reason, evidence, operator_telegram_user_id) values ($1, $2, 'not_spam', 'confirmed_normal', 'owner_review', $3, $4, $5)",
    )
    .bind(chat_id)
    .bind(user_id)
    .bind(reason)
    .bind(evidence)
    .bind(operator_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("update telegram_chat_users set is_spammer = false, spam_score = 0, spam_last_marked_at = null, spam_reason = null, spam_type = null, spam_types = coalesce(spam_types, '{}'::jsonb) - 'llm_generic_comment', updated_at = now() where chat_id = $1 and telegram_user_id = $2")
        .bind(chat_id)
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("update telegram_messages set spam_marked_at = null, spam_reason = null, spam_source = null where chat_id = $1 and user_id = $2 and source_channel_id is null and spam_source = 'manual_owner_confirmation'")
        .bind(chat_id)
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
