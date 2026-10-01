use sqlx::PgPool;
use teloxide::prelude::*;

use crate::telegram::html::{Html, bold, code, lines, paragraphs};
use crate::telegram::service_messages::{self, MessageAudience};

pub async fn send_memory_notes(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    audience: Option<MessageAudience>,
) -> ResponseResult<()> {
    let Some(audience) = audience else {
        return Ok(());
    };
    let notes = sqlx::query_as::<_, (i32, Option<String>, Vec<String>, String)>(
        r#"
        select source_message_id, summary, entities, status
        from post_history_entries
        order by created_at desc
        limit 5
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|err| {
        tracing::error!(%err, "failed to load memory notes");
        teloxide::RequestError::Io(std::io::Error::other("memory check failed").into())
    })?;

    if notes.is_empty() {
        service_messages::send_html(bot, chat_id, "Память пока пустая.", audience).await?;
        return Ok(());
    }

    let text = paragraphs(notes.into_iter().map(
        |(source_message_id, summary, entities, status)| {
            lines([
                bold(format!("Пост {source_message_id}")),
                Html::text(summary.unwrap_or_else(|| "Без RAG-карточки".to_string())),
                code(format!("{} · {}", status, entities.join(", "))),
            ])
        },
    ))
    .into_string();

    service_messages::send_html(bot, chat_id, text, audience).await?;

    Ok(())
}
