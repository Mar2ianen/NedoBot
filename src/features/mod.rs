#[allow(dead_code)]
pub mod ask_metrics;
pub mod chat_search;
// Wired into the dispatcher after the agent and Telegram handler slices land.
#[cfg(feature = "ask")]
#[allow(dead_code)]
pub mod ask;
// The production bot binary does not start MCP transports; their entry-point binaries do.
// Keep the shared catalog compiled there without masking diagnostics in its implementation.
#[cfg(feature = "moderation")]
pub mod auto_moderation;
#[allow(dead_code)]
pub mod chat_read_api;
pub mod chat_retrieval;
#[cfg(feature = "emoji")]
pub mod emoji_analysis;
#[cfg(feature = "auto-comment")]
pub mod first_comment;
pub mod health;
pub mod ingest;
pub mod jobs;
#[cfg(feature = "moderation")]
pub mod labels;
#[cfg(feature = "manual-moderation")]
pub mod manual_moderation;
pub mod member_greetings;
pub mod memory;
#[cfg(feature = "moderation")]
pub mod new_user_analysis;
#[cfg(feature = "moderation")]
pub mod new_user_audit;
#[cfg(feature = "moderation")]
pub mod qr_url;
#[cfg(feature = "moderation")]
pub mod reports;
#[cfg(feature = "moderation")]
pub mod risk_captcha;
pub mod search;
pub mod service_message_cleanup;
#[cfg(feature = "spam-sync")]
pub mod spam_reputation;
#[cfg(feature = "moderation")]
pub mod spam_review;
pub mod spammer_avatar_embeddings;
pub mod stats;
/// Только локальное сопоставление; загрузка списка и политика остаются в приложении.
#[cfg(feature = "moderation")]
pub mod url_feeds;
#[cfg(feature = "moderation")]
pub mod url_relations;
pub mod user_profiles;
#[cfg(feature = "voice")]
pub mod voice;

#[cfg(feature = "moderation")]
pub mod decision_audit;

#[cfg(feature = "moderation")]
pub mod url_shadow_projection;
