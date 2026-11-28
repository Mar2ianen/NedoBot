use sqlx::PgPool;
use teloxide::types::Message;

use crate::config::Config;
use crate::db::telegram::{MessagePersistenceOutcome, save_telegram_message};
use crate::telegram::entities::TelegramMessageRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressSkipReason {
    UnmanagedChat,
    IngestDisabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressOutcome {
    Ignored {
        message: TelegramMessageRef,
        reason: IngressSkipReason,
    },
    Persisted(MessagePersistenceOutcome),
}

impl IngressOutcome {
    pub fn was_persisted(self) -> bool {
        matches!(self, Self::Persisted(_))
    }
}

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
pub async fn ingest_message(
    pool: &PgPool,
    msg: &Message,
    config: &Config,
) -> anyhow::Result<IngressOutcome> {
    let message = TelegramMessageRef::from_message(msg);
    if !is_managed_chat(config, msg.chat.id.0) {
        return Ok(IngressOutcome::Ignored {
            message,
            reason: IngressSkipReason::UnmanagedChat,
        });
    }
    if !managed_chat_allows(config, msg.chat.id.0, |chat| chat.ingest) {
        return Ok(IngressOutcome::Ignored {
            message,
            reason: IngressSkipReason::IngestDisabled,
        });
    }
    let persistence = save_telegram_message(pool, msg, config).await?;
    #[cfg(feature = "flood")]
    if config.flood_shadow_enabled
        && let Err(error) = crate::features::flood_shadow::observe_chat(
            pool,
            msg.chat.id.0,
            msg.thread_id.as_ref().map(|thread| thread.0.0),
        )
        .await
    {
        tracing::warn!(
            chat_id = msg.chat.id.0,
            message_id = msg.id.0,
            %error,
            "flood shadow observation failed"
        );
    }
    #[cfg(feature = "moderation")]
    if let Err(_error) = crate::features::decision_audit::service::observe_message(
        pool,
        config,
        msg.chat.id.0,
        msg.id.0,
    )
    .await
    {
        // Ошибка наблюдения не останавливает основной ingest и локальную модерацию.
        tracing::warn!(
            chat_id = msg.chat.id.0,
            message_id = msg.id.0,
            "cheap moderation observation failed"
        );
    }
    Ok(IngressOutcome::Persisted(persistence))
}

#[cfg(test)]
mod tests {
    use super::{IngressOutcome, IngressSkipReason, ingest_message};
    use crate::telegram::entities::TelegramMessageRef;
    use sqlx::postgres::PgPoolOptions;
    use teloxide::types::Message;

    #[tokio::test]
    async fn disabled_ingest_skips_before_any_database_effect() {
        let mut config = crate::config::test_config();
        let chat = config
            .community
            .chats
            .get_mut("main")
            .expect("test community has a primary chat");
        let chat_id = chat.id;
        chat.ingest = false;
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .expect("lazy test pool should be constructible without connecting");
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 17,
            "date": 1700000000,
            "chat": {"id": chat_id, "type": "supergroup", "title": "Test"},
            "from": {"id": 42, "is_bot": false, "first_name": "A"},
            "text": "must not be persisted"
        }))
        .expect("synthetic Telegram message should deserialize");

        let outcome = ingest_message(&pool, &message, &config)
            .await
            .expect("disabled ingestion must return before connecting to PostgreSQL");

        assert_eq!(
            outcome,
            IngressOutcome::Ignored {
                message: TelegramMessageRef {
                    chat_id,
                    message_id: 17,
                },
                reason: IngressSkipReason::IngestDisabled,
            }
        );
        pool.close().await;
    }

    #[test]
    fn ingress_outcomes_preserve_identity_and_distinguish_skip_reasons() {
        let message = TelegramMessageRef {
            chat_id: -100_000_000_001,
            message_id: 17,
        };
        let unknown = IngressOutcome::Ignored {
            message,
            reason: IngressSkipReason::UnmanagedChat,
        };
        let disabled = IngressOutcome::Ignored {
            message,
            reason: IngressSkipReason::IngestDisabled,
        };

        assert_eq!(
            unknown,
            IngressOutcome::Ignored {
                message,
                reason: IngressSkipReason::UnmanagedChat,
            }
        );
        assert_eq!(
            disabled,
            IngressOutcome::Ignored {
                message,
                reason: IngressSkipReason::IngestDisabled,
            }
        );
        assert!(!unknown.was_persisted());
        assert!(!disabled.was_persisted());
    }
}
