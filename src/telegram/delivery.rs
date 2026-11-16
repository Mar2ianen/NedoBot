//! Typed, narrow effects for durable Telegram message delivery.
//!
//! Request construction and message size validation stay in the Telegram adapters;
//! durable callers receive only a safe error kind and delivery certainty.

use async_trait::async_trait;
use teloxide::{
    adaptors::DefaultParseMode,
    errors::{ApiError, RequestError},
    payloads::SendDocumentSetters,
    prelude::{Bot, ChatId, Message, Requester, ResponseResult},
    types::{InputFile, InputRichMessage, MessageId, ReplyParameters},
};

use crate::telegram::{render::send_rich_message_reply, service_messages};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryErrorKind {
    InvalidInput,
    Configuration,
    RateLimited,
    PayloadRejected,
    TelegramRejected,
    Transport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryFailure {
    pub kind: DeliveryErrorKind,
    pub retry_after_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryEffect<T> {
    NotAttempted(DeliveryFailure),
    Rejected(DeliveryFailure),
    Unknown(DeliveryFailure),
    Confirmed(T),
}

impl<T> DeliveryEffect<T> {
    pub fn certainty(&self) -> DeliveryCertainty {
        match self {
            Self::NotAttempted(_) => DeliveryCertainty::NotAttempted,
            Self::Rejected(_) => DeliveryCertainty::Rejected,
            Self::Unknown(_) => DeliveryCertainty::Unknown,
            Self::Confirmed(_) => DeliveryCertainty::Confirmed,
        }
    }

    pub fn failure(&self) -> Option<DeliveryFailure> {
        match self {
            Self::NotAttempted(failure) | Self::Rejected(failure) | Self::Unknown(failure) => {
                Some(*failure)
            }
            Self::Confirmed(_) => None,
        }
    }

    pub fn map<TNext>(self, map: impl FnOnce(T) -> TNext) -> DeliveryEffect<TNext> {
        match self {
            Self::NotAttempted(failure) => DeliveryEffect::NotAttempted(failure),
            Self::Rejected(failure) => DeliveryEffect::Rejected(failure),
            Self::Unknown(failure) => DeliveryEffect::Unknown(failure),
            Self::Confirmed(value) => DeliveryEffect::Confirmed(map(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryCertainty {
    NotAttempted,
    Rejected,
    Unknown,
    Confirmed,
}

impl DeliveryFailure {
    fn of(kind: DeliveryErrorKind) -> Self {
        Self {
            kind,
            retry_after_seconds: None,
        }
    }
}

pub fn classify_request_result<T>(result: ResponseResult<T>) -> DeliveryEffect<T> {
    match result {
        Ok(message) => DeliveryEffect::Confirmed(message),
        Err(RequestError::Validation(_) | RequestError::Serialization(_)) => {
            DeliveryEffect::NotAttempted(DeliveryFailure::of(DeliveryErrorKind::InvalidInput))
        }
        Err(RequestError::RetryAfter(seconds)) => DeliveryEffect::Rejected(DeliveryFailure {
            kind: DeliveryErrorKind::RateLimited,
            retry_after_seconds: Some(i64::from(seconds.seconds())),
        }),
        Err(RequestError::Api(ApiError::InvalidToken)) => {
            DeliveryEffect::Rejected(DeliveryFailure::of(DeliveryErrorKind::Configuration))
        }
        Err(RequestError::Api(ApiError::CantParseEntities(_) | ApiError::MessageIsTooLong)) => {
            DeliveryEffect::Rejected(DeliveryFailure::of(DeliveryErrorKind::PayloadRejected))
        }
        Err(RequestError::Api(ApiError::Unknown(description)))
            if description.starts_with("Bad Request:") =>
        {
            DeliveryEffect::Rejected(DeliveryFailure::of(DeliveryErrorKind::PayloadRejected))
        }
        Err(RequestError::Api(_) | RequestError::MigrateToChatId(_)) => {
            DeliveryEffect::Rejected(DeliveryFailure::of(DeliveryErrorKind::TelegramRejected))
        }
        Err(RequestError::Network(_) | RequestError::InvalidJson { .. } | RequestError::Io(_)) => {
            DeliveryEffect::Unknown(DeliveryFailure::of(DeliveryErrorKind::Transport))
        }
    }
}

/// Capabilities used by durable comment and voice delivery. Lifecycle and retry policy remain
/// with each feature; this port only sends one typed Telegram request at a time.
#[async_trait]
pub trait TelegramSendPort: Send + Sync {
    async fn send_html_reply(
        &self,
        chat_id: ChatId,
        reply_to_message_id: MessageId,
        html: String,
    ) -> DeliveryEffect<Message>;

    async fn send_rich_message_reply(
        &self,
        chat_id: ChatId,
        reply_to_message_id: MessageId,
        message: InputRichMessage,
    ) -> DeliveryEffect<Message>;

    async fn send_document_reply(
        &self,
        chat_id: ChatId,
        reply_to_message_id: MessageId,
        filename: String,
        body: String,
    ) -> DeliveryEffect<Message>;
}

pub struct BotTelegramSendPort<'a> {
    bot: &'a DefaultParseMode<Bot>,
}

impl<'a> BotTelegramSendPort<'a> {
    pub fn new(bot: &'a DefaultParseMode<Bot>) -> Self {
        Self { bot }
    }
}

#[async_trait]
impl TelegramSendPort for BotTelegramSendPort<'_> {
    async fn send_html_reply(
        &self,
        chat_id: ChatId,
        reply_to_message_id: MessageId,
        html: String,
    ) -> DeliveryEffect<Message> {
        classify_request_result(
            service_messages::send_html_reply(
                self.bot,
                chat_id,
                reply_to_message_id,
                html,
                service_messages::MessageAudience::Public,
            )
            .await,
        )
    }

    async fn send_rich_message_reply(
        &self,
        chat_id: ChatId,
        reply_to_message_id: MessageId,
        message: InputRichMessage,
    ) -> DeliveryEffect<Message> {
        classify_request_result(
            send_rich_message_reply(self.bot, chat_id, reply_to_message_id, message).await,
        )
    }

    async fn send_document_reply(
        &self,
        chat_id: ChatId,
        reply_to_message_id: MessageId,
        filename: String,
        body: String,
    ) -> DeliveryEffect<Message> {
        let result = self
            .bot
            .send_document(InputFile::memory(body.into_bytes()).file_name(filename))
            .chat_id(chat_id)
            .reply_parameters(
                ReplyParameters::new(reply_to_message_id).allow_sending_without_reply(),
            )
            .await;
        classify_request_result(result)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    struct FakeTelegramSendPort {
        outcomes: Mutex<VecDeque<DeliveryEffect<Message>>>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl FakeTelegramSendPort {
        fn new(outcomes: impl IntoIterator<Item = DeliveryEffect<Message>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn call(&self, operation: &'static str) -> DeliveryEffect<Message> {
            self.calls.lock().unwrap().push(operation);
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .expect("test must provide a result for every send")
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl TelegramSendPort for FakeTelegramSendPort {
        async fn send_html_reply(
            &self,
            _: ChatId,
            _: MessageId,
            _: String,
        ) -> DeliveryEffect<Message> {
            self.call("html")
        }

        async fn send_rich_message_reply(
            &self,
            _: ChatId,
            _: MessageId,
            _: InputRichMessage,
        ) -> DeliveryEffect<Message> {
            self.call("rich")
        }

        async fn send_document_reply(
            &self,
            _: ChatId,
            _: MessageId,
            _: String,
            _: String,
        ) -> DeliveryEffect<Message> {
            self.call("document")
        }
    }

    fn failure(kind: DeliveryErrorKind, retry_after_seconds: Option<i64>) -> DeliveryFailure {
        DeliveryFailure {
            kind,
            retry_after_seconds,
        }
    }

    #[tokio::test]
    async fn fake_port_injects_unknown_without_implicitly_retrying() {
        let port = FakeTelegramSendPort::new([DeliveryEffect::Unknown(failure(
            DeliveryErrorKind::Transport,
            None,
        ))]);

        let result = port
            .send_html_reply(ChatId(-100), MessageId(7), "safe".to_owned())
            .await;

        assert_eq!(result.certainty(), DeliveryCertainty::Unknown);
        assert_eq!(port.calls(), ["html"]);
    }

    #[tokio::test]
    async fn fake_port_distinguishes_not_attempted_and_confirmed_rejection() {
        let port = FakeTelegramSendPort::new([
            DeliveryEffect::NotAttempted(failure(DeliveryErrorKind::InvalidInput, None)),
            DeliveryEffect::Rejected(failure(DeliveryErrorKind::RateLimited, Some(45))),
        ]);

        let not_attempted = port
            .send_html_reply(ChatId(-100), MessageId(7), "bad".to_owned())
            .await;
        let rejected = port
            .send_html_reply(ChatId(-100), MessageId(7), "retry-after".to_owned())
            .await;

        assert_eq!(not_attempted.certainty(), DeliveryCertainty::NotAttempted);
        assert_eq!(rejected.certainty(), DeliveryCertainty::Rejected);
        assert_eq!(rejected.failure().unwrap().retry_after_seconds, Some(45));
        assert_eq!(port.calls(), ["html", "html"]);
    }

    #[test]
    fn request_error_classification_keeps_delivery_certainty_and_safe_kinds() {
        let validation = RequestError::Validation(
            teloxide::requests::validation::RequestValidationError::InvalidValue {
                path: teloxide::requests::validation::RequestFieldPath::field("text"),
                reason: teloxide::requests::validation::InvalidValueReason::MustBeInRange {
                    min: 1,
                    max: 4096,
                },
            },
        );
        assert_eq!(
            classify_request_result::<()>(Err(validation)).certainty(),
            DeliveryCertainty::NotAttempted
        );
        assert_eq!(
            classify_request_result::<()>(Err(RequestError::RetryAfter(
                teloxide::types::Seconds::from_seconds(30),
            ))),
            DeliveryEffect::Rejected(failure(DeliveryErrorKind::RateLimited, Some(30)))
        );
        let unknown = RequestError::Io(std::sync::Arc::new(std::io::Error::other(
            "private payload",
        )));
        assert_eq!(
            classify_request_result::<()>(Err(unknown)),
            DeliveryEffect::Unknown(failure(DeliveryErrorKind::Transport, None))
        );
    }
}
