use teloxide::{
    adaptors::DefaultParseMode,
    payloads::{SendMessageSetters, SendRichMessageSetters},
    prelude::{Bot, ChatId, Message, Requester, ResponseResult},
    types::{EphemeralMessageParameters, InputRichMessage, MessageId, UserId},
};

use crate::telegram::render::{disabled_link_preview, normalize_rich_text, normalize_send_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageAudience {
    Public,
    Ephemeral(UserId),
}

/// Selects a private command response only for managed group chats that enabled it.
/// In private chats, Telegram's ordinary bot conversation already has one recipient.
pub fn command_audience(message: &Message, ephemeral_enabled: bool) -> MessageAudience {
    if ephemeral_enabled && !message.chat.is_private() {
        message
            .from
            .as_ref()
            .map(|user| MessageAudience::Ephemeral(user.id))
            .unwrap_or(MessageAudience::Public)
    } else {
        MessageAudience::Public
    }
}

pub async fn send_html(
    bot: &DefaultParseMode<Bot>,
    chat_id: ChatId,
    text: impl Into<String>,
    audience: MessageAudience,
) -> ResponseResult<Message> {
    let text = normalize_send_text(text)?;
    match audience {
        MessageAudience::Public => {
            bot.send_message(chat_id, text)
                .link_preview_options(disabled_link_preview())
                .await
        }
        MessageAudience::Ephemeral(receiver_user_id) if chat_id.0 < 0 => {
            bot.send_message(chat_id, text)
                .ephemeral_message_parameters(EphemeralMessageParameters::new(receiver_user_id))
                .link_preview_options(disabled_link_preview())
                .await
        }
        MessageAudience::Ephemeral(_) => {
            bot.send_message(chat_id, text)
                .link_preview_options(disabled_link_preview())
                .await
        }
    }
}

pub async fn send_html_reply(
    bot: &DefaultParseMode<Bot>,
    chat_id: ChatId,
    reply_to_message_id: MessageId,
    text: impl Into<String>,
    audience: MessageAudience,
) -> ResponseResult<Message> {
    let text = normalize_send_text(text)?;
    match audience {
        MessageAudience::Public => {
            bot.send_message(chat_id, text)
                .reply_parameters(
                    teloxide::types::ReplyParameters::new(reply_to_message_id)
                        .allow_sending_without_reply(),
                )
                .link_preview_options(disabled_link_preview())
                .await
        }
        // Ephemeral replies cannot target an ordinary message. They remain private to the
        // command author, while the original command stays visible in the group.
        MessageAudience::Ephemeral(receiver_user_id) if chat_id.0 < 0 => {
            bot.send_message(chat_id, text)
                .ephemeral_message_parameters(EphemeralMessageParameters::new(receiver_user_id))
                .link_preview_options(disabled_link_preview())
                .await
        }
        MessageAudience::Ephemeral(_) => {
            bot.send_message(chat_id, text)
                .reply_parameters(
                    teloxide::types::ReplyParameters::new(reply_to_message_id)
                        .allow_sending_without_reply(),
                )
                .link_preview_options(disabled_link_preview())
                .await
        }
    }
}

pub async fn send_rich_html(
    bot: &DefaultParseMode<Bot>,
    chat_id: ChatId,
    html: impl Into<String>,
    audience: MessageAudience,
) -> ResponseResult<Message> {
    let message = InputRichMessage::html(normalize_rich_text(html)?);
    match audience {
        MessageAudience::Public => bot.send_rich_message(chat_id, message).await,
        MessageAudience::Ephemeral(receiver_user_id) if chat_id.0 < 0 => {
            bot.send_rich_message(chat_id, message)
                .ephemeral_message_parameters(EphemeralMessageParameters::new(receiver_user_id))
                .await
        }
        MessageAudience::Ephemeral(_) => bot.send_rich_message(chat_id, message).await,
    }
}
