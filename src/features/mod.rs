// Wired into the dispatcher after the agent and Telegram handler slices land.
#[allow(dead_code)]
pub mod ask;
pub mod avatar_analysis;
// The production bot binary does not start MCP transports; their entry-point binaries do.
// Keep the shared catalog compiled there without masking diagnostics in its implementation.
#[allow(dead_code)]
pub mod chat_read_api;
pub mod chat_retrieval;
pub mod first_comment;
pub mod first_message_spam;
pub mod memory;
pub mod new_user_analysis;
pub mod search;
pub mod spam_review;
pub mod stats;
pub mod user_profiles;
pub mod voice;
