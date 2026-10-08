use teloxide::{
    adaptors::DefaultParseMode,
    errors::{ApiError, RequestError},
    payloads::{SendMessageSetters, SendRichMessageSetters},
    prelude::{Bot, ChatId, Message, Requester, ResponseResult},
    types::{
        EphemeralMessageParameters, InputRichMessage, MessageId, ReplyParameters, ThreadId, UserId,
    },
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
    let sender = message.from.as_ref();
    audience_for_command(
        ephemeral_enabled,
        message.chat.is_private(),
        sender.map(|user| user.id),
        sender.is_some_and(|user| user.is_bot),
        message.sender_chat.is_some(),
        message.thread_id,
    )
}

fn audience_for_command(
    ephemeral_enabled: bool,
    is_private_chat: bool,
    sender: Option<UserId>,
    sender_is_bot: bool,
    sender_chat_present: bool,
    message_thread_id: Option<ThreadId>,
) -> Option<MessageAudience> {
    if !ephemeral_enabled || is_private_chat {
        return Some(MessageAudience::Public);
    }
    let receiver_user_id = sender?;
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
    let original_text = text.into();
    let text = match normalize_send_text(original_text.clone()) {
        Ok(text) => text,
        Err(error) => {
            return send_plain_fallback(bot, chat_id, None, &original_text, audience, error).await;
        }
    };
    let result = match audience {
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
    };
    match result {
        Ok(sent) => Ok(sent),
        Err(error) if should_fallback_after_payload_rejection(&error) => {
            send_plain_fallback(bot, chat_id, None, &original_text, audience, error).await
        }
        Err(error) => Err(error),
    }
}

pub async fn send_html_reply(
    bot: &DefaultParseMode<Bot>,
    chat_id: ChatId,
    reply_to_message_id: MessageId,
    text: impl Into<String>,
    audience: MessageAudience,
) -> ResponseResult<Message> {
    let original_text = text.into();
    let text = match normalize_send_text(original_text.clone()) {
        Ok(text) => text,
        Err(error) => {
            return send_plain_fallback(
                bot,
                chat_id,
                Some(reply_to_message_id),
                &original_text,
                audience,
                error,
            )
            .await;
        }
    };
    let result = match audience {
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
    };
    match result {
        Ok(sent) => Ok(sent),
        Err(error) if should_fallback_after_payload_rejection(&error) => {
            send_plain_fallback(
                bot,
                chat_id,
                Some(reply_to_message_id),
                &original_text,
                audience,
                error,
            )
            .await
        }
        Err(error) => Err(error),
    }
}

async fn send_plain_fallback(
    bot: &DefaultParseMode<Bot>,
    chat_id: ChatId,
    reply_to_message_id: Option<MessageId>,
    html: &str,
    audience: MessageAudience,
    original_error: RequestError,
) -> ResponseResult<Message> {
    let plain = html_to_plain_text(html);
    let plain = match normalize_send_text(plain) {
        Ok(plain) => plain,
        Err(_) => return Err(original_error),
    };
    tracing::warn!(
        %original_error,
        chat_id = chat_id.0,
        "Telegram HTML delivery failed; retrying as escaped plain text"
    );
    let bot = bot.inner();
    let request = bot
        .send_message(chat_id, plain)
        .link_preview_options(disabled_link_preview());
    match audience {
        MessageAudience::Public => match reply_to_message_id {
            Some(message_id) => {
                request
                    .reply_parameters(
                        ReplyParameters::new(message_id).allow_sending_without_reply(),
                    )
                    .await
            }
            None => request.await,
        },
        MessageAudience::Ephemeral {
            receiver_user_id,
            message_thread_id,
        } if chat_id.0 < 0 => {
            let request = with_ephemeral_parameters(request, receiver_user_id, message_thread_id);
            request.await
        }
        MessageAudience::Ephemeral { .. } => match reply_to_message_id {
            Some(message_id) => {
                request
                    .reply_parameters(
                        ReplyParameters::new(message_id).allow_sending_without_reply(),
                    )
                    .await
            }
            None => request.await,
        },
    }
}

pub(crate) fn html_to_plain_text(html: &str) -> String {
    let mut characters = html.chars().peekable();
    let mut plain = String::with_capacity(html.len());
    while let Some(character) = characters.next() {
        match character {
            '<' => {
                let mut tag = String::new();
                for next in characters.by_ref() {
                    if next == '>' {
                        break;
                    }
                    tag.push(next);
                }
                let tag = tag.trim().to_ascii_lowercase();
                if tag.starts_with("br")
                    || tag.starts_with("/p")
                    || tag.starts_with("/h")
                    || tag.starts_with("/blockquote")
                    || tag.starts_with("/div")
                {
                    plain.push('\n');
                }
            }
            '&' => {
                let mut entity = String::new();
                while let Some(&next) = characters.peek() {
                    if next == ';' || entity.chars().count() >= 8 {
                        break;
                    }
                    entity.push(next);
                    characters.next();
                }
                if characters.next_if_eq(&';').is_some() {
                    match entity.as_str() {
                        "amp" => plain.push('&'),
                        "lt" => plain.push('<'),
                        "gt" => plain.push('>'),
                        "quot" => plain.push('"'),
                        "#39" => plain.push('\''),
                        _ => {
                            plain.push('&');
                            plain.push_str(&entity);
                            plain.push(';');
                        }
                    }
                } else {
                    plain.push('&');
                    plain.push_str(&entity);
                }
            }
            _ => plain.push(character),
        }
    }
    plain.trim().to_owned()
}

pub async fn send_rich_html(
    bot: &DefaultParseMode<Bot>,
    chat_id: ChatId,
    html: impl Into<String>,
    audience: MessageAudience,
) -> ResponseResult<Message> {
    let html = normalize_rich_text(html)?;
    let message = InputRichMessage::html(html.clone());
    let result = match audience {
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
    };

    match result {
        Ok(sent) => Ok(sent),
        Err(error) if should_fallback_after_payload_rejection(&error) => {
            tracing::warn!(%error, chat_id = chat_id.0, "Telegram rejected rich message; retrying with HTML fallback");
            send_html(bot, chat_id, html, audience).await
        }
        Err(error) => Err(error),
    }
}

/// A rich-send retry is safe only when Telegram (or local request validation)
/// confirmed that it rejected the payload. Network and response parsing errors
/// can mean the original message was accepted, so callers must not send again.
pub fn should_fallback_after_payload_rejection(error: &RequestError) -> bool {
    match error {
        RequestError::Validation(_) => true,
        RequestError::Api(ApiError::CantParseEntities(_) | ApiError::MessageIsTooLong) => true,
        RequestError::Api(ApiError::Unknown(description)) => {
            description.starts_with("Bad Request:")
        }
        RequestError::RetryAfter(_)
        | RequestError::Api(_)
        | RequestError::MigrateToChatId(_)
        | RequestError::Network(_)
        | RequestError::InvalidJson { .. }
        | RequestError::Io(_) => false,
    }
}

/// A failed send with no definitive Telegram rejection may already have been
/// accepted. Durable senders should not retry it as a new message.
pub fn delivery_outcome_is_unknown(error: &RequestError) -> bool {
    matches!(error, RequestError::Network(_) | RequestError::Io(_))
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
    use super::{
        MessageAudience, audience_for_command, delivery_outcome_is_unknown,
        should_fallback_after_payload_rejection, with_ephemeral_parameters,
    };
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::{TcpListener, TcpStream},
        thread,
    };
    use teloxide::{
        errors::{ApiError, RequestError},
        prelude::{Bot, ChatId, Requester, RequesterExt},
        requests::HasPayload,
        types::{MessageId, ParseMode, ThreadId, UserId},
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
            audience_for_command(true, false, None, false, false, thread_id),
            None
        );
        assert_eq!(
            audience_for_command(true, false, Some(UserId(42)), true, false, thread_id),
            None
        );
        assert_eq!(
            audience_for_command(true, false, Some(UserId(42)), false, true, thread_id),
            None
        );
    }

    #[test]
    fn command_audience_retains_the_forum_topic_id() {
        let thread_id = ThreadId(MessageId(22));
        assert_eq!(
            audience_for_command(true, false, Some(UserId(42)), false, false, Some(thread_id),),
            Some(MessageAudience::Ephemeral {
                receiver_user_id: UserId(42),
                message_thread_id: Some(thread_id),
            })
        );
    }

    #[test]
    fn private_or_disabled_commands_keep_public_audience_without_sender_metadata() {
        assert_eq!(
            audience_for_command(false, false, None, false, false, None),
            Some(MessageAudience::Public)
        );
        assert_eq!(
            audience_for_command(true, true, None, false, false, None),
            Some(MessageAudience::Public)
        );
    }

    #[test]
    fn rich_html_fallback_only_runs_after_a_confirmed_payload_rejection() {
        assert!(should_fallback_after_payload_rejection(&RequestError::Api(
            ApiError::Unknown("Bad Request: invalid rich message".to_owned())
        )));
        assert!(should_fallback_after_payload_rejection(&RequestError::Api(
            ApiError::CantParseEntities("Bad Request: can't parse entities".to_owned())
        )));
        assert!(!should_fallback_after_payload_rejection(
            &RequestError::RetryAfter(teloxide::types::Seconds::from_seconds(30))
        ));
        assert!(!should_fallback_after_payload_rejection(
            &RequestError::Api(ApiError::BotBlocked)
        ));
    }

    #[test]
    fn html_fallback_preserves_readable_text_and_decodes_entities_without_markup() {
        assert_eq!(
            super::html_to_plain_text(
                "<b>Имя &amp; &lt;текст&gt;</b><br><a href=\"tg://user?id=42\">Профиль</a>"
            ),
            "Имя & <текст>\nПрофиль"
        );
    }

    #[tokio::test]
    async fn confirmed_html_rejection_retries_as_plain_text_without_exposing_ephemeral_reply() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock Telegram API");
        let address = listener.local_addr().expect("mock API address");
        let server = thread::spawn(move || {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept Telegram request");
                let payload = read_request_payload(&stream);
                if request_index == 0 {
                    let body = r#"{"ok":false,"error_code":400,"description":"Bad Request: can't parse entities"}"#;
                    write_json_response(&mut stream, "400 Bad Request", body);
                    continue;
                }

                assert_eq!(payload["text"], "Hello & <world>");
                assert!(payload.get("parse_mode").is_none());
                assert_eq!(payload["message_thread_id"], 77);
                assert_eq!(
                    payload["ephemeral_message_parameters"]["receiver_user_id"],
                    42
                );
                assert!(payload.get("reply_parameters").is_none());

                let body = serde_json::json!({
                    "ok": true,
                    "result": {
                        "message_id": 5,
                        "date": 1,
                        "chat": {"id": -1001, "type": "supergroup", "title": "Test"},
                        "text": "Hello & <world>"
                    }
                })
                .to_string();
                write_json_response(&mut stream, "200 OK", &body);
            }
        });
        let bot = Bot::new("123456:TEST_TOKEN")
            .set_api_url(format!("http://{address}/").parse().expect("mock URL"))
            .parse_mode(ParseMode::Html);
        let thread_id = ThreadId(MessageId(77));

        let sent = super::send_html(
            &bot,
            ChatId(-1001),
            "<b>Hello &amp; &lt;world&gt;</b>",
            MessageAudience::Ephemeral {
                receiver_user_id: UserId(42),
                message_thread_id: Some(thread_id),
            },
        )
        .await
        .expect("plain text fallback should be delivered");

        server.join().expect("mock server thread");
        assert_eq!(sent.id, MessageId(5));
    }

    fn read_request_payload(stream: &TcpStream) -> serde_json::Value {
        let mut reader = BufReader::new(stream.try_clone().expect("clone request stream"));
        let mut content_length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("read request header");
            if line == "\r\n" || line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                content_length = value.trim().parse::<usize>().expect("content length");
            }
        }
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).expect("read request body");
        serde_json::from_slice(&body).expect("Telegram JSON request")
    }

    fn write_json_response(stream: &mut TcpStream, status: &str, body: &str) {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("write mock Telegram response");
        stream.flush().expect("flush mock Telegram response");
    }

    #[test]
    fn ambiguous_transport_failures_are_not_safe_to_retry_as_new_messages() {
        let io_error = RequestError::Io(std::sync::Arc::new(std::io::Error::other("test")));
        assert!(delivery_outcome_is_unknown(&io_error));
        assert!(!delivery_outcome_is_unknown(&RequestError::RetryAfter(
            teloxide::types::Seconds::from_seconds(30)
        )));
        assert!(!delivery_outcome_is_unknown(&RequestError::Api(
            ApiError::Unknown("Internal Server Error".to_owned())
        )));
    }
}
