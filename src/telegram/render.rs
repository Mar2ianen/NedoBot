use teloxide::{
    prelude::*,
    types::{LinkPreviewOptions, MessageId, ReplyParameters},
};

pub async fn send_html(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    text: impl Into<String>,
) -> ResponseResult<Message> {
    bot.send_message(chat_id, text.into())
        .link_preview_options(LinkPreviewOptions {
            is_disabled: true,
            url: None,
            prefer_small_media: false,
            prefer_large_media: false,
            show_above_text: false,
        })
        .await
}

pub async fn send_html_reply(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    reply_to_message_id: MessageId,
    text: impl Into<String>,
) -> ResponseResult<Message> {
    bot.send_message(chat_id, text.into())
        .reply_parameters(ReplyParameters::new(reply_to_message_id).allow_sending_without_reply())
        .link_preview_options(LinkPreviewOptions {
            is_disabled: true,
            url: None,
            prefer_small_media: false,
            prefer_large_media: false,
            show_above_text: false,
        })
        .await
}

pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
