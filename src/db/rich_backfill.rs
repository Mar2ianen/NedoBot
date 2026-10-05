//! Восстановление текста rich-сообщений без повторной обработки апдейтов.
use crate::telegram::entities::{message_has_links, message_text};
use sqlx::PgPool;
use teloxide::types::Message;

const PAGE_SIZE: i64 = 200;

#[derive(Debug, Default)]
pub struct BackfillSummary {
    pub candidates: u64,
    pub repaired: u64,
    pub unreadable: u64,
}

pub async fn backfill_rich_messages(
    pool: &PgPool,
    chat_id: i64,
    apply: bool,
) -> anyhow::Result<BackfillSummary> {
    let mut summary = BackfillSummary::default();
    let mut after_id = i32::MIN;
    loop {
        let rows: Vec<(i32, serde_json::Value)> = sqlx::query_as(
            "select message_id, raw_json from telegram_messages where chat_id = $1 and message_id > $2 and nullif(btrim(text), '') is null and raw_json ? 'rich_message' order by message_id limit $3"
        ).bind(chat_id).bind(after_id).bind(PAGE_SIZE).fetch_all(pool).await?;
        if rows.is_empty() {
            break;
        }
        for (message_id, raw_json) in rows {
            after_id = message_id;
            summary.candidates += 1;
            let Ok(message) = serde_json::from_value::<Message>(raw_json.clone()) else {
                summary.unreadable += 1;
                continue;
            };
            // Не переносим текст из повреждённого payload чужого чата/сообщения.
            if message.chat.id.0 != chat_id || message.id.0 != message_id {
                summary.unreadable += 1;
                continue;
            }
            let Some(text) = message_text(&message) else {
                continue;
            };
            if !apply {
                summary.repaired += 1;
                continue;
            }
            let affected = sqlx::query(
                "update telegram_messages set text = $3, has_links = $4, updated_at = now() where chat_id = $1 and message_id = $2 and nullif(btrim(text), '') is null and raw_json = $5"
            ).bind(chat_id).bind(message_id).bind(text.as_ref()).bind(message_has_links(&message)).bind(raw_json)
                .execute(pool).await?.rows_affected();
            summary.repaired += affected;
        }
    }
    Ok(summary)
}
