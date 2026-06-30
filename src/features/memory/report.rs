use sqlx::PgPool;
use teloxide::prelude::*;

use crate::telegram::render::{escape_html, send_html};

pub async fn send_memory_notes(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
) -> ResponseResult<()> {
    let notes = sqlx::query_as::<_, (String, String, String)>(
        r#"
        select title, summary, array_to_string(keywords, ', ')
        from post_memory_notes
        order by created_at desc
        limit 5
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|err| {
        tracing::error!(%err, "failed to load memory notes");
        teloxide::RequestError::Io(std::io::Error::other("memory check failed"))
    })?;

    if notes.is_empty() {
        bot.send_message(chat_id, "Память пока пустая.").await?;
        return Ok(());
    }

    let text = notes
        .into_iter()
        .map(|(title, summary, keywords)| {
            format!(
                "<b>{}</b>\n{}\n<code>{}</code>",
                escape_html(&title),
                escape_html(&summary),
                escape_html(&keywords)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    send_html(bot, chat_id, text).await?;

    Ok(())
}
