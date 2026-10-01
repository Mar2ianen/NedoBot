use teloxide::{
    adaptors::DefaultParseMode,
    payloads::{SendMessageSetters, SendRichMessageSetters},
    prelude::{Bot, ChatId, Message, Requester, ResponseResult},
    types::{EphemeralMessageParameters, InputRichMessage, MessageId, ThreadId, UserId},
};

use crate::telegram::render::{disabled_link_preview, normalize_rich_text, normalize_send_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageAudience {
    Public,
    Ephemeral {
        receiver_user_id: UserId,
        message_thread_id: Option<ThreadId>,
    },
}

/// Selects a private command response only for managed group chats that enabled it.
/// In private chats, Telegram's ordinary bot conversation already has one recipient.
pub fn command_audience(message: &Message, ephemeral_enabled: bool) -> Option<MessageAudience> {
    let sender = message
        .from
        .as_ref()
        .filter(|user| !user.is_bot && message.sender_chat.is_none())
        .map(|user| (user.id, user.is_bot));
    audience_for_command(
        ephemeral_enabled,
        message.chat.is_private(),
        sender,
        message.sender_chat.is_some(),
        message.thread_id,
    )
}

fn audience_for_command(
    ephemeral_enabled: bool,
    is_private_chat: bool,
    sender: Option<(UserId, bool)>,
    sender_chat_present: bool,
    message_thread_id: Option<ThreadId>,
) -> Option<MessageAudience> {
    if !ephemeral_enabled || is_private_chat {
        return Some(MessageAudience::Public);
    }
    let Some((receiver_user_id, sender_is_bot)) = sender else {
        return None;
    };
    if sender_is_bot || sender_chat_present {
        return None;
    }
    Some(MessageAudience::Ephemeral {
        receiver_user_id,
        message_thread_id,
    })
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
        MessageAudience::Ephemeral {
            receiver_user_id,
            message_thread_id,
        } if chat_id.0 < 0 => {
            with_ephemeral_parameters(
                bot.send_message(chat_id, text),
                receiver_user_id,
                message_thread_id,
            )
            .link_preview_options(disabled_link_preview())
            .await
        }
        MessageAudience::Ephemeral { .. } => {
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
        MessageAudience::Ephemeral {
            receiver_user_id,
            message_thread_id,
        } if chat_id.0 < 0 => {
            with_ephemeral_parameters(
                bot.send_message(chat_id, text),
                receiver_user_id,
                message_thread_id,
            )
            .link_preview_options(disabled_link_preview())
            .await
        }
        MessageAudience::Ephemeral { .. } => {
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
        MessageAudience::Ephemeral {
            receiver_user_id,
            message_thread_id,
        } if chat_id.0 < 0 => {
            let request = bot
                .send_rich_message(chat_id, message)
                .ephemeral_message_parameters(EphemeralMessageParameters::new(receiver_user_id));
            let request = match message_thread_id {
                Some(thread_id) => request.message_thread_id(thread_id),
                None => request,
            };
            request.await
        }
        MessageAudience::Ephemeral { .. } => bot.send_rich_message(chat_id, message).await,
    }
}

fn with_ephemeral_parameters<R: SendMessageSetters>(
    request: R,
    receiver_user_id: UserId,
    message_thread_id: Option<ThreadId>,
) -> R {
    let request =
        request.ephemeral_message_parameters(EphemeralMessageParameters::new(receiver_user_id));
    match message_thread_id {
        Some(thread_id) => request.message_thread_id(thread_id),
        None => request,
    }
}

#[cfg(test)]
mod tests {
    use super::{MessageAudience, audience_for_command, with_ephemeral_parameters};
    use teloxide::{
        prelude::{Bot, ChatId, Requester},
        requests::HasPayload,
        types::{MessageId, ThreadId, UserId},
    };

    #[test]
    fn ephemeral_send_keeps_receiver_and_forum_topic_without_a_public_reply_target() {
        let receiver_user_id = UserId(42);
        let thread_id = ThreadId(MessageId(987));
        let request = with_ephemeral_parameters(
            Bot::new("123456:TEST_TOKEN").send_message(ChatId(-1001), "private result"),
            receiver_user_id,
            Some(thread_id),
        );
        let payload = request.payload_ref();

        assert_eq!(payload.message_thread_id, Some(thread_id));
        assert_eq!(payload.reply_parameters, None);
        assert_eq!(
            payload
                .ephemeral_message_parameters
                .as_ref()
                .map(|parameters| parameters.receiver_user_id),
            Some(receiver_user_id)
        );
    }

    #[test]
    fn enabled_ephemeral_replies_suppress_messages_without_a_real_user_sender() {
        let thread_id = Some(ThreadId(MessageId(22)));
        assert_eq!(
            audience_for_command(true, false, None, false, thread_id),
            None
        );
        assert_eq!(
            audience_for_command(true, false, Some((UserId(42), true)), false, thread_id),
            None
        );
        assert_eq!(
            audience_for_command(true, false, Some((UserId(42), false)), true, thread_id),
            None
        );
    }

    #[test]
    fn command_audience_retains_the_forum_topic_id() {
        let thread_id = ThreadId(MessageId(22));
        assert_eq!(
            audience_for_command(
                true,
                false,
                Some((UserId(42), false)),
                false,
                Some(thread_id),
            ),
            Some(MessageAudience::Ephemeral {
                receiver_user_id: UserId(42),
                message_thread_id: Some(thread_id),
            })
        );
    }

    #[test]
    fn private_or_disabled_commands_keep_public_audience_without_sender_metadata() {
        assert_eq!(
            audience_for_command(false, false, None, false, None),
            Some(MessageAudience::Public)
        );
        assert_eq!(
            audience_for_command(true, true, None, false, None),
            Some(MessageAudience::Public)
        );
    }
}
