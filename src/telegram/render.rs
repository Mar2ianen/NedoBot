use std::time::Duration;

use serde::{Deserialize, Serialize};
use teloxide::{
    prelude::*,
    types::{LinkPreviewOptions, MessageId, ReplyParameters},
};

use crate::{
    http,
    telegram::html::{self, TELEGRAM_TEXT_LIMIT, is_safe_len},
};

#[allow(dead_code)]
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

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
pub struct InputRichMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    markdown: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_rtl: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skip_entity_detection: Option<bool>,
}

#[allow(dead_code)]
impl InputRichMessage {
    pub fn html(content: impl Into<String>) -> ResponseResult<Self> {
        Ok(Self {
            html: Some(normalize_rich_text(content)?),
            markdown: None,
            is_rtl: None,
            skip_entity_detection: None,
        })
    }

    pub fn markdown(content: impl Into<String>) -> ResponseResult<Self> {
        Ok(Self {
            html: None,
            markdown: Some(normalize_rich_text(content)?),
            is_rtl: None,
            skip_entity_detection: None,
        })
    }

    pub fn rtl(mut self, is_rtl: bool) -> Self {
        self.is_rtl = Some(is_rtl);
        self
    }

    pub fn skip_entity_detection(mut self, skip_entity_detection: bool) -> Self {
        self.skip_entity_detection = Some(skip_entity_detection);
        self
    }
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
struct SendRichMessageRequest {
    chat_id: i64,
    rich_message: InputRichMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_parameters: Option<ReplyParameters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disable_notification: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    protect_content: Option<bool>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct TelegramApiResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
    error_code: Option<u16>,
}

#[allow(dead_code)]
pub async fn send_rich_html(chat_id: ChatId, html: impl Into<String>) -> ResponseResult<Message> {
    send_rich_message(chat_id, InputRichMessage::html(html)?).await
}

#[allow(dead_code)]
pub async fn send_rich_markdown(
    chat_id: ChatId,
    markdown: impl Into<String>,
) -> ResponseResult<Message> {
    send_rich_message(chat_id, InputRichMessage::markdown(markdown)?).await
}

#[allow(dead_code)]
pub async fn send_rich_html_reply(
    chat_id: ChatId,
    reply_to_message_id: MessageId,
    html: impl Into<String>,
) -> ResponseResult<Message> {
    send_rich_message_reply(chat_id, reply_to_message_id, InputRichMessage::html(html)?).await
}

#[allow(dead_code)]
pub async fn send_rich_markdown_reply(
    chat_id: ChatId,
    reply_to_message_id: MessageId,
    markdown: impl Into<String>,
) -> ResponseResult<Message> {
    send_rich_message_reply(
        chat_id,
        reply_to_message_id,
        InputRichMessage::markdown(markdown)?,
    )
    .await
}

#[allow(dead_code)]
pub async fn send_rich_message(
    chat_id: ChatId,
    rich_message: InputRichMessage,
) -> ResponseResult<Message> {
    send_rich_message_request(SendRichMessageRequest {
        chat_id: chat_id.0,
        rich_message,
        reply_parameters: None,
        disable_notification: None,
        protect_content: None,
    })
    .await
}

#[allow(dead_code)]
pub async fn send_rich_message_reply(
    chat_id: ChatId,
    reply_to_message_id: MessageId,
    rich_message: InputRichMessage,
) -> ResponseResult<Message> {
    send_rich_message_request(SendRichMessageRequest {
        chat_id: chat_id.0,
        rich_message,
        reply_parameters: Some(
            ReplyParameters::new(reply_to_message_id).allow_sending_without_reply(),
        ),
        disable_notification: None,
        protect_content: None,
    })
    .await
}

#[allow(dead_code)]
async fn send_rich_message_request(request: SendRichMessageRequest) -> ResponseResult<Message> {
    let token = telegram_token()?;
    let url = format!("https://api.telegram.org/bot{token}/sendRichMessage");
    let response = http::client(Duration::from_secs(15))
        .map_err(io_request_error)?
        .post(url)
        .json(&request)
        .send()
        .await
        .map_err(|_| io_request_error("sendRichMessage request failed"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|_| io_request_error("sendRichMessage response read failed"))?;

    let api_response: TelegramApiResponse<Message> =
        serde_json::from_str(&body).map_err(|err| {
            io_request_error(format!(
                "failed to parse sendRichMessage response: {err}; status={status}"
            ))
        })?;

    if api_response.ok {
        api_response.result.ok_or_else(|| {
            io_request_error(format!(
                "sendRichMessage response is ok but result is missing; status={status}"
            ))
        })
    } else {
        Err(io_request_error(format!(
            "sendRichMessage failed: error_code={:?}; description={}; status={status}",
            api_response.error_code,
            api_response
                .description
                .unwrap_or_else(|| "unknown Telegram API error".to_owned())
        )))
    }
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

#[allow(dead_code)]
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

#[allow(dead_code)]
fn telegram_token() -> ResponseResult<String> {
    std::env::var("TELOXIDE_TOKEN")
        .or_else(|_| std::env::var("BOT_TOKEN"))
        .map_err(|_| {
            io_request_error("TELOXIDE_TOKEN or BOT_TOKEN is required for sendRichMessage")
        })
}

fn io_request_error(error: impl std::fmt::Display) -> teloxide::RequestError {
    teloxide::RequestError::Io(std::io::Error::other(error.to_string()))
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
