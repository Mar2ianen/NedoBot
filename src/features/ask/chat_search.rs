//! Backward-compatible facade for the shared chat read-model.
//!
//! Query implementation lives in `features::chat_read_api::service` so MCP
//! transports and in-process callers use identical SQL and presentation rules.
#[allow(unused_imports)]
pub use crate::features::chat_read_api::service::{
    count_messages, message_context, message_url, recent_messages, reply_thread, search_messages,
    source_id, user_interactions, user_profile,
};
#[allow(unused_imports)]
pub use crate::features::chat_read_api::types::{
    ChatInteraction, ChatMessage, ChatReadScope, ChatUserProfile, MessageMatch, MessageSearchPage,
    MessageSearchRequest, MessageSort, RecentMessagesRequest,
};
