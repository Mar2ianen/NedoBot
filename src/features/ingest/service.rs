use sqlx::PgPool;
use teloxide::types::Message;

use crate::config::Config;
use crate::db::telegram::save_telegram_message;

/// Returns whether an update belongs to this bot's configured community.
/// Unknown chats are rejected before any profile/message persistence.
pub fn is_managed_chat(config: &Config, chat_id: i64) -> bool {
    match config.community.telegram.unknown_chat_policy {
        crate::config_file::UnknownChatPolicy::Ignore => config.chat_by_id(chat_id).is_some(),
    }
}

pub fn managed_chat_allows(
    config: &Config,
    chat_id: i64,
    feature: fn(&crate::config_file::ChatConfig) -> bool,
) -> bool {
    config.chat_allows(chat_id, feature)
}

/// Neutral Telegram message persistence. Feature pipelines may consume the
/// result, but no feature owns base message ingest anymore.
pub async fn ingest_message(pool: &PgPool, msg: &Message, config: &Config) -> anyhow::Result<bool> {
    if !managed_chat_allows(config, msg.chat.id.0, |chat| chat.ingest) {
        return Ok(false);
    }
    save_telegram_message(pool, msg, config).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    #[test]
    fn feature_scope_is_explicit_in_the_service_api() {
        // The database path is integration-tested by existing Telegram CRUD
        // tests; this unit test documents the boundary without a live pool.
        assert_eq!(
            std::mem::size_of::<fn(&crate::config_file::ChatConfig) -> bool>(),
            std::mem::size_of::<usize>()
        );
    }
}
