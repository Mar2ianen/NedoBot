//! Восстановление текста rich-сообщений без повторной обработки апдейтов.
use crate::telegram::entities::{message_has_links, message_text};
use crate::telegram::export_rich;
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
            let Some((text, has_links)) = stored_rich_text(&raw_json, chat_id, message_id) else {
                summary.unreadable += 1;
                continue;
            };
            if text.is_empty() {
                continue;
            }
            if !apply {
                summary.repaired += 1;
                continue;
            }
            let affected = sqlx::query(
                "update telegram_messages set text = $3, has_links = has_links or $4, updated_at = now() where chat_id = $1 and message_id = $2 and nullif(btrim(text), '') is null and raw_json = $5"
            ).bind(chat_id).bind(message_id).bind(text).bind(has_links).bind(raw_json)
                .execute(pool).await?.rows_affected();
            summary.repaired += affected;
        }
    }
    Ok(summary)
}

fn stored_rich_text(
    raw: &serde_json::Value,
    chat_id: i64,
    message_id: i32,
) -> Option<(String, bool)> {
    if raw.get("message_id").is_some() || raw.get("chat").is_some() {
        let message: Message = serde_json::from_value(raw.clone()).ok()?;
        // Не переносим текст из повреждённого native payload чужого чата/сообщения.
        if message.chat.id.0 != chat_id || message.id.0 != message_id {
            return None;
        }
        return Some((
            message_text(&message).unwrap_or_default().into_owned(),
            message_has_links(&message),
        ));
    }
    // Desktop export не содержит chat ID внутри сообщения: scope задаётся
    // явным chat_id при импорте и backfill. Проверяем ID и признаки export schema.
    if raw["id"].as_i64() != Some(i64::from(message_id))
        || raw["type"].as_str() != Some("message")
        || raw["date_unixtime"]
            .as_str()
            .and_then(|value| value.parse::<i64>().ok())
            .is_none()
    {
        return None;
    }
    let projection = export_rich::project(raw.get("rich_message")?);
    Some((projection.text, projection.has_links))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn desktop_scope_checks_id_and_cannot_bypass_native_chat_validation() {
        let mut raw = json!({"id":5,"type":"message","date_unixtime":"1","rich_message":{"blocks":[
            {"type":"paragraph","text":{"type":"plain","text":"Текст"}}
        ]}});
        assert_eq!(stored_rich_text(&raw, -1001, 5).unwrap().0, "Текст");
        assert!(stored_rich_text(&raw, -1001, 6).is_none());
        raw["chat"] = json!({"id":-1002,"type":"supergroup","title":"Чужой чат"});
        assert!(stored_rich_text(&raw, -1001, 5).is_none());
    }
}
