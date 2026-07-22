// Wired into the dispatcher after the agent and Telegram handler slices land.
#[allow(dead_code)]
pub mod ask;
pub mod avatar_analysis;
pub mod first_comment;
pub mod first_message_spam;
pub mod memory;
pub mod new_user_analysis;
pub mod search;
pub mod spam_review;
pub mod stats;
pub mod user_profiles;
pub mod voice;
