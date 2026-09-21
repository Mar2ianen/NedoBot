#[allow(dead_code)]
pub mod ask_metrics;
pub mod chat_search;
// Wired into the dispatcher after the agent and Telegram handler slices land.
#[cfg(feature = "ask")]
#[allow(dead_code)]
pub mod ask;
// The production bot binary does not start MCP transports; their entry-point binaries do.
// Keep the shared catalog compiled there without masking diagnostics in its implementation.
#[allow(dead_code)]
pub mod chat_read_api;
pub mod chat_retrieval;
#[cfg(feature = "auto-comment")]
pub mod first_comment;
pub mod ingest;
pub mod jobs;
pub mod memory;
#[cfg(feature = "moderation")]
pub mod new_user_analysis;
#[cfg(feature = "moderation")]
pub mod new_user_audit;
pub mod search;
#[cfg(feature = "moderation")]
pub mod spam_review;
pub mod stats;
pub mod user_profiles;
#[cfg(feature = "voice")]
pub mod voice;
