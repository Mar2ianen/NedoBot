//! Публичные typed DTO read-model.
//!
//! Пока DTO остаются в `service`, чтобы сохранить прежний API `chat_search`
//! без изменений. Этот модуль зарезервирован для общих transport-neutral типов.
pub use super::service::{
    ChatInteraction, ChatMessage, ChatUserProfile, MessageMatch, MessageSearchRequest, MessageSort,
    RecentMessagesRequest,
};
