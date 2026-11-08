//! Transport-neutral DTOs and the reviewed read-model scope.

use serde::{Deserialize, Serialize};
use sqlx::{
    FromRow,
    types::chrono::{DateTime, Utc},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatReadScope {
    pub(crate) instance_id: String,
    pub(crate) discussion_chat_id: i64,
    pub(crate) source_channel_id: Option<i64>,
}

impl ChatReadScope {
    pub fn new(
        instance_id: impl Into<String>,
        discussion_chat_id: i64,
        source_channel_id: Option<i64>,
    ) -> anyhow::Result<Self> {
        let instance_id = instance_id.into();
        anyhow::ensure!(
            !instance_id.trim().is_empty(),
            "MCP instance id must not be empty"
        );
        anyhow::ensure!(
            discussion_chat_id != 0,
            "MCP discussion chat id must not be zero"
        );
        anyhow::ensure!(
            source_channel_id.is_none_or(|chat_id| chat_id != 0 && chat_id != discussion_chat_id),
            "MCP source channel id must be non-zero and differ from the discussion chat id"
        );
        Ok(Self {
            instance_id,
            discussion_chat_id,
            source_channel_id,
        })
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn discussion_chat_id(&self) -> i64 {
        self.discussion_chat_id
    }

    pub fn source_channel_id(&self) -> Option<i64> {
        self.source_channel_id
    }
}

#[cfg(test)]
mod scope_tests {
    use super::ChatReadScope;

    #[test]
    fn scope_rejects_invalid_ids_and_blank_instance_names() {
        assert!(ChatReadScope::new("", -1001, None).is_err());
        assert!(ChatReadScope::new("test", 0, None).is_err());
        assert!(ChatReadScope::new("test", -1001, Some(0)).is_err());
        assert!(ChatReadScope::new("test", -1001, Some(-1001)).is_err());
    }

    #[test]
    fn scope_accepts_chat_without_source_channel() {
        let scope = ChatReadScope::new("community_a", -2001, None).unwrap();
        assert_eq!(scope.instance_id, "community_a");
        assert_eq!(scope.discussion_chat_id, -2001);
        assert_eq!(scope.source_channel_id, None);
    }
}

pub const CHAT_EMBEDDING_URL_ENV: &str = "ASK_CHAT_EMBEDDING_URL";
pub const CHAT_EMBEDDING_MODEL_ENV: &str = "ASK_CHAT_EMBEDDING_MODEL";
pub const CHAT_EMBEDDING_TIMEOUT_ENV: &str = "ASK_CHAT_EMBEDDING_TIMEOUT_SEC";
pub const CHAT_EMBEDDING_QUERY_PREFIX_ENV: &str = "ASK_CHAT_EMBEDDING_QUERY_PREFIX";

/// Session advisory lock shared by running MCP servers and taken exclusively
/// by scope provisioning. Its value is also used by the additive DB migration.
pub const MCP_INSTANCE_SCOPE_LOCK_KEY: i64 = 5_567_381_882_901_844_037;

#[derive(Clone, Debug)]
pub struct SemanticSearchConfig {
    pub embedding_url: String,
    pub embedding_model: String,
    pub timeout_sec: u64,
    pub query_prefix: String,
}

/// Telegram file metadata for a single public-chat photo or document.
/// This DTO stays inside the MCP server and is never serialized to public tools.
#[derive(Clone, Debug, FromRow)]
pub struct ChatMediaAttachment {
    pub message_id: i32,
    pub media_kind: String,
    pub file_id: String,
    pub file_size: Option<i64>,
    pub file_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MessageMatch {
    Hybrid,
    FullText,
    AnyTerms,
    Literal,
    WholeWord,
}

impl MessageMatch {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Hybrid => "hybrid",
            Self::FullText => "full_text",
            Self::AnyTerms => "any_terms",
            Self::Literal => "literal",
            Self::WholeWord => "whole_word",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MessageSort {
    Relevance,
    Newest,
    Oldest,
}

impl MessageSort {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Newest => "newest",
            Self::Oldest => "oldest",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct MessageSearchRequest {
    pub query: String,
    pub user_id: Option<i64>,
    pub date_from: Option<DateTime<Utc>>,
    pub date_to: Option<DateTime<Utc>>,
    pub reply_to_message_id: Option<i32>,
    pub is_automatic_forward: Option<bool>,
    pub is_forwarded: Option<bool>,
    pub has_reply: Option<bool>,
    pub has_links: Option<bool>,
    pub has_media: Option<bool>,
    pub has_photo: Option<bool>,
    pub has_video: Option<bool>,
    pub has_document: Option<bool>,
    pub has_audio: Option<bool>,
    pub has_voice: Option<bool>,
    pub has_sticker: Option<bool>,
    pub has_animation: Option<bool>,
    pub match_mode: MessageMatch,
    pub sort: MessageSort,
    pub limit: i64,
    /// Zero-based page offset. The transport validates and bounds it before
    /// constructing this request.
    pub offset: i64,
    pub include_forwards: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MessageSearchPage {
    pub messages: Vec<ChatMessage>,
    pub total_count: i64,
    pub has_more: bool,
    pub next_offset: Option<i64>,
    pub scan_limit_reached: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ChatMessage {
    pub message_id: i32,
    pub user_id: Option<i64>,
    /// Явное имя автора для модели; `user_id` остаётся стабильным ключом.
    pub author_name: String,
    /// Совместимый alias `author_name` для существующих потребителей.
    pub author: String,
    pub author_url: Option<String>,
    pub is_forwarded: bool,
    pub forwarded_from: Option<String>,
    pub text: String,
    pub reply_to_message_id: Option<i32>,
    pub created_at: String,
    pub relevance: i32,
    pub source_id: String,
    pub message_url: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ChatInteraction {
    pub message: ChatMessage,
    pub replied_to: Option<ChatMessage>,
}

#[derive(Clone, Debug)]
pub struct RecentMessagesRequest {
    pub user_id: Option<i64>,
    pub date_from: Option<DateTime<Utc>>,
    pub date_to: Option<DateTime<Utc>>,
    pub is_automatic_forward: Option<bool>,
    pub is_forwarded: Option<bool>,
    pub has_reply: Option<bool>,
    pub has_links: Option<bool>,
    pub has_media: Option<bool>,
    pub has_photo: Option<bool>,
    pub has_video: Option<bool>,
    pub has_document: Option<bool>,
    pub has_audio: Option<bool>,
    pub has_voice: Option<bool>,
    pub has_sticker: Option<bool>,
    pub has_animation: Option<bool>,
    pub sort: MessageSort,
    pub limit: i64,
    pub include_forwards: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, sqlx::FromRow)]
pub struct ChatUserProfile {
    pub telegram_user_id: i64,
    pub username: Option<String>,
    pub display_name: String,
    pub author_url: Option<String>,
    pub bio: Option<String>,
    pub is_bot: bool,
    pub is_premium: Option<bool>,
    pub language_code: Option<String>,
    pub message_count: i64,
    pub message_rank: i64,
    pub reply_count: i64,
    pub link_count: i64,
    pub media_count: i64,
    pub first_seen_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub member_status: Option<String>,
    pub is_admin: bool,
    pub admin_title: Option<String>,
    pub is_present: Option<bool>,
}
