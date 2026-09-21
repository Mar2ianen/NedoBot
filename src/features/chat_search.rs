//! Shared facade for the chat read-model.
//!
//! Chat search is used by both `/ask` and first-comment. Keeping it outside
//! either optional feature lets moderation-only and voice-only binaries avoid
//! pulling the ask feature merely to compile common read-model types.
#[allow(unused_imports)]
pub use crate::features::chat_read_api::service::{
    count_messages, message_context, message_url, recent_messages, reply_thread, search_messages,
    source_id, user_interactions, user_profile,
};
#[allow(unused_imports)]
pub use crate::features::chat_read_api::types::{
    ChatInteraction, ChatMessage, ChatReadScope, ChatUserProfile, MessageMatch, MessageSearchPage,
    MessageSearchRequest, MessageSort, RecentMessagesRequest, SemanticSearchConfig,
};
