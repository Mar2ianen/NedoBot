use teloxide::{
    prelude::*,
    types::{LinkPreviewOptions, MessageId, ReplyParameters},
};

use crate::telegram::html::{self, TELEGRAM_TEXT_LIMIT, is_safe_len};

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

pub fn escape_html(text: &str) -> String {
    html::escape(text)
}

fn normalize_send_text(text: impl Into<String>) -> ResponseResult<String> {
    let text = text.into();
    let text = if text.trim().is_empty() {
        "Пустой ответ.".to_string()
    } else {
        text
    };

    let char_count = text.chars().count();
    if char_count > TELEGRAM_TEXT_LIMIT {
        return Err(teloxide::RequestError::Io(std::io::Error::other(format!(
            "HTML message exceeds Telegram text limit: {char_count}/{TELEGRAM_TEXT_LIMIT}"
        ))));
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

fn disabled_link_preview() -> LinkPreviewOptions {
    LinkPreviewOptions {
        is_disabled: true,
        url: None,
        prefer_small_media: false,
        prefer_large_media: false,
        show_above_text: false,
    }
}
