//! Transport-neutral DTOs and the reviewed read-model scope.

use serde::{Deserialize, Serialize};
use sqlx::types::chrono::{DateTime, Utc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChatReadScope {
    pub discussion_chat_id: i64,
    pub source_channel_id: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
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

#[derive(Clone, Debug)]
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
