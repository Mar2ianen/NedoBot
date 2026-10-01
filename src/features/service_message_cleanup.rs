use teloxide::{adaptors::DefaultParseMode, prelude::*, types::Message};

use crate::config::Config;

/// Deletes Telegram join/leave notices in chats that explicitly enable cleanup.
/// Deletion is best-effort so a missing Telegram permission does not interrupt
/// normal message ingestion or other handlers.
pub async fn maybe_delete_join_leave_message(
    bot: &DefaultParseMode<Bot>,
    msg: &Message,
    config: &Config,
) {
    let cleanup_enabled = config.chat_allows(msg.chat.id.0, |chat| chat.delete_join_leave_messages);
    if !cleanup_enabled || !is_join_leave_service_message(msg) {
        return;
    }

    match bot.delete_message(msg.chat.id, msg.id).await {
        Ok(_) => tracing::debug!(
            chat_id = msg.chat.id.0,
            message_id = msg.id.0,
            "deleted Telegram join/leave service message"
        ),
        Err(err) => tracing::warn!(
            %err,
            chat_id = msg.chat.id.0,
            message_id = msg.id.0,
            "failed to delete Telegram join/leave service message"
        ),
    }
}

fn is_join_leave_service_message(msg: &Message) -> bool {
    msg.new_chat_members().is_some() || msg.left_chat_member().is_some()
}
