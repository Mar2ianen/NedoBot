use teloxide::{
    RequestError,
    adaptors::DefaultParseMode,
    drafter::{
        CleanupFailure, DraftId, DrafterBackend, DrafterCapabilities, DrafterErrorClass,
        DrafterOperation, DrafterRateLimitKey, NativeRichBackend, PreviewAck,
        StatusThenRichBackend,
    },
    prelude::{Bot, ChatId, UserId},
    types::{InputRichMessage, MessageId, ReplyParameters},
};

/// Rich `/ask` delivery with a Telegram-native draft in private chats.
///
/// Telegram does not support native drafts in group chats, so groups use a
/// temporary status message that is edited in place and removed after the
/// permanent answer is delivered. Keeping both modes behind one backend keeps
/// the scheduler and the `/ask` lifecycle identical.
pub enum AskDrafterBackend {
    Native(Box<NativeRichBackend<DefaultParseMode<Bot>>>),
    Status(Box<StatusThenRichBackend<DefaultParseMode<Bot>>>),
}

impl AskDrafterBackend {
    #[must_use]
    pub fn new(
        bot: DefaultParseMode<Bot>,
        chat_id: ChatId,
        user_id: UserId,
        use_native_draft: bool,
        reply_parameters: ReplyParameters,
    ) -> Self {
        if use_native_draft {
            Self::Native(Box::new(
                NativeRichBackend::new(bot, user_id).reply_parameters(reply_parameters),
            ))
        } else {
            Self::Status(Box::new(
                StatusThenRichBackend::new(bot, chat_id).reply_parameters(reply_parameters),
            ))
        }
    }
}

impl DrafterBackend for AskDrafterBackend {
    type Preview = InputRichMessage;
    type Final = InputRichMessage;
    type SegmentOutput = teloxide::types::Message;
    type Output = teloxide::types::Message;
    type Error = RequestError;

    fn capabilities(&self) -> DrafterCapabilities {
        match self {
            Self::Native(backend) => backend.capabilities(),
            Self::Status(backend) => backend.capabilities(),
        }
    }

    fn rate_limit_key(&self) -> DrafterRateLimitKey {
        match self {
            Self::Native(backend) => backend.rate_limit_key(),
            Self::Status(backend) => backend.rate_limit_key(),
        }
    }

    fn draft_id(&self) -> Option<DraftId> {
        match self {
            Self::Native(backend) => backend.draft_id(),
            Self::Status(backend) => backend.draft_id(),
        }
    }

    fn preview_message_id(&self) -> Option<MessageId> {
        match self {
            Self::Native(backend) => backend.preview_message_id(),
            Self::Status(backend) => backend.preview_message_id(),
        }
    }

    async fn update(&mut self, preview: Self::Preview) -> Result<PreviewAck, Self::Error> {
        match self {
            Self::Native(backend) => backend.update(preview).await,
            Self::Status(backend) => backend.update(status_preview_text(&preview)).await,
        }
    }

    async fn commit_segment(
        &mut self,
        final_payload: &Self::Final,
    ) -> Result<Self::SegmentOutput, Self::Error> {
        match self {
            Self::Native(backend) => backend.commit_segment(final_payload).await,
            Self::Status(backend) => backend.commit_segment(final_payload).await,
        }
    }

    async fn finish(&mut self, final_payload: &Self::Final) -> Result<Self::Output, Self::Error> {
        match self {
            Self::Native(backend) => backend.finish(final_payload).await,
            Self::Status(backend) => backend.finish(final_payload).await,
        }
    }

    async fn abort(&mut self) -> Result<(), Self::Error> {
        match self {
            Self::Native(backend) => backend.abort().await,
            Self::Status(backend) => backend.abort().await,
        }
    }

    fn classify_error(
        &self,
        operation: DrafterOperation,
        error: &Self::Error,
    ) -> DrafterErrorClass {
        match self {
            Self::Native(backend) => backend.classify_error(operation, error),
            Self::Status(backend) => backend.classify_error(operation, error),
        }
    }

    fn take_cleanup_failure(&mut self) -> Option<CleanupFailure<Self::Error>> {
        match self {
            Self::Native(backend) => backend.take_cleanup_failure(),
            Self::Status(backend) => backend.take_cleanup_failure(),
        }
    }
}

fn status_preview_text(preview: &InputRichMessage) -> String {
    preview
        .markdown_ref()
        .or_else(|| preview.html_ref())
        .unwrap_or_default()
        .to_owned()
}
