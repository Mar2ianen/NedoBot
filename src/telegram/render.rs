use teloxide::{
    prelude::*,
    types::{InputRichMessage, LinkPreviewOptions, MessageId, ReplyParameters},
};

use crate::telegram::html::{self, TELEGRAM_TEXT_LIMIT, is_safe_len};

const TELEGRAM_RICH_TEXT_LIMIT: usize = 32_768;

pub async fn send_html(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    text: impl Into<String>,
) -> ResponseResult<Message> {
    let text = normalize_send_text(text)?;

    bot.send_message(chat_id, text)
        .link_preview_options(disabled_link_preview())
        .await
}

pub async fn send_html_reply(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    reply_to_message_id: MessageId,
    text: impl Into<String>,
) -> ResponseResult<Message> {
    let text = normalize_send_text(text)?;

    bot.send_message(chat_id, text)
        .reply_parameters(ReplyParameters::new(reply_to_message_id).allow_sending_without_reply())
        .link_preview_options(disabled_link_preview())
        .await
}

pub async fn send_rich_html(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    html: impl Into<String>,
) -> ResponseResult<Message> {
    send_rich_message(
        bot,
        chat_id,
        InputRichMessage::html(normalize_rich_text(html)?),
    )
    .await
}

pub fn validate_rich_markdown(markdown: &str) -> ResponseResult<()> {
    normalize_rich_text(markdown.to_owned()).map(|_| ())
}

pub async fn send_rich_message(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    rich_message: InputRichMessage,
) -> ResponseResult<Message> {
    bot.send_rich_message(chat_id, rich_message).await
}

pub async fn send_rich_message_reply(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    reply_to_message_id: MessageId,
    rich_message: InputRichMessage,
) -> ResponseResult<Message> {
    bot.send_rich_message(chat_id, rich_message)
        .reply_parameters(ReplyParameters::new(reply_to_message_id).allow_sending_without_reply())
        .await
}

pub fn escape_html(text: &str) -> String {
    html::escape(text)
}

fn normalize_send_text(text: impl Into<String>) -> ResponseResult<String> {
    let text = normalize_non_empty_text(text);

    let char_count = text.chars().count();
    if char_count > TELEGRAM_TEXT_LIMIT {
        return Err(io_request_error(format!(
            "HTML message exceeds Telegram text limit: {char_count}/{TELEGRAM_TEXT_LIMIT}"
        )));
    }

    if !is_safe_len(&text) {
        tracing::warn!(
            chars = char_count,
            telegram_limit = TELEGRAM_TEXT_LIMIT,
            "HTML message is close to or above Telegram text limit"
        );
    }

    Ok(text)
}

fn normalize_rich_text(text: impl Into<String>) -> ResponseResult<String> {
    let text = normalize_non_empty_text(text);
    let char_count = text.chars().count();

    if char_count > TELEGRAM_RICH_TEXT_LIMIT {
        return Err(io_request_error(format!(
            "rich message exceeds Telegram rich text limit: {char_count}/{TELEGRAM_RICH_TEXT_LIMIT}"
        )));
    }

    Ok(text)
}

fn normalize_non_empty_text(text: impl Into<String>) -> String {
    let text = text.into();
    if text.trim().is_empty() {
        "Пустой ответ.".to_string()
    } else {
        text
    }
}

fn io_request_error(error: impl std::fmt::Display) -> teloxide::RequestError {
    teloxide::RequestError::Io(std::io::Error::other(error.to_string()).into())
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

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Timestamp;
    use teloxide::utils::time::{DateTimeFormat, DateTimeToken, TimeContext};

    #[test]
    fn link_previews_are_disabled_for_every_text_send_path() {
        let options = disabled_link_preview();

        assert!(options.is_disabled);
        assert!(options.url.is_none());
        assert!(!options.prefer_small_media);
        assert!(!options.prefer_large_media);
        assert!(!options.show_above_text);
    }

    #[test]
    fn rich_markdown_input_keeps_telegram_limit() {
        assert!(validate_rich_markdown("## Ответ").is_ok());
        assert!(validate_rich_markdown(&"x".repeat(TELEGRAM_RICH_TEXT_LIMIT + 1)).is_err());
    }

    #[test]
    fn generated_time_entities_pass_production_rich_validation() {
        let context = TimeContext::from_name("Europe/Moscow").unwrap();
        let instant = "2026-08-03T11:00:00Z".parse::<Timestamp>().unwrap();
        let examples = [
            DateTimeToken::instant_in(&context, instant, DateTimeFormat::Time).to_markdown(),
            DateTimeToken::instant_in(&context, instant, DateTimeFormat::Date).to_markdown(),
            DateTimeToken::instant_in(&context, instant, DateTimeFormat::DateTime).to_markdown(),
        ];
        for entity in examples {
            assert!(validate_rich_markdown(&entity).is_ok(), "entity: {entity}");
            assert!(validate_rich_markdown(&format!("**Время:** {entity}")).is_ok());
        }
    }
}
