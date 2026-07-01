use sqlx::PgPool;
use teloxide::prelude::*;

use crate::telegram::html::{Html, bold, code, lines, paragraphs};
use crate::telegram::render::send_html;

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

    let text = paragraphs(notes.into_iter().map(|(title, summary, keywords)| {
        lines([bold(title), Html::text(summary), code(keywords)])
    }))
    .into_string();

    send_html(bot, chat_id, text).await?;

    Ok(())
}
