use crate::telegram::entities::utf16_range;
use std::future::Future;
use teloxide::types::{Message, MessageEntity, MessageEntityKind};

use crate::state::TelegramBotIdentity;
use sqlx::PgPool;

/// Drops an in-flight generation immediately when its database lease is lost.
pub async fn run_while_claimed<F>(
    future: F,
    mut lease_lost: tokio::sync::oneshot::Receiver<()>,
) -> Option<F::Output>
where
    F: Future,
{
    tokio::pin!(future);
    tokio::select! {
        biased;
        _ = &mut lease_lost => None,
        result = &mut future => Some(result),
    }
}

pub async fn claim_update(
    pool: &PgPool,
    instance_id: &str,
    chat_id: i64,
    message_id: i32,
) -> anyhow::Result<Option<uuid::Uuid>> {
    sqlx::query(
        "delete from ask_gateway_invocations where completed_at is null and lease_until < now() - interval '1 day'",
    )
    .execute(pool)
    .await?;
    let claim_token = uuid::Uuid::new_v4();
    let claimed = sqlx::query_scalar::<_, uuid::Uuid>(
        r#"insert into ask_gateway_invocations (instance_id, chat_id, message_id, claim_token, lease_until)
           values ($1, $2, $3, $4, now() + interval '15 minutes')
           on conflict (instance_id, chat_id, message_id) do update
             set claim_token = excluded.claim_token,
                 lease_until = now() + interval '15 minutes'
             where ask_gateway_invocations.completed_at is null
               and ask_gateway_invocations.lease_until <= now()
           returning claim_token"#,
    )
    .bind(instance_id)
    .bind(chat_id)
    .bind(message_id)
    .bind(claim_token)
    .fetch_optional(pool)
    .await?;
    Ok(claimed)
}

pub async fn complete_update(
    pool: &PgPool,
    instance_id: &str,
    chat_id: i64,
    message_id: i32,
    claim_token: uuid::Uuid,
) -> anyhow::Result<()> {
    sqlx::query("update ask_gateway_invocations set completed_at = now() where instance_id = $1 and chat_id = $2 and message_id = $3 and claim_token = $4 and completed_at is null")
        .bind(instance_id).bind(chat_id).bind(message_id).bind(claim_token).execute(pool).await?;
    Ok(())
}

pub async fn renew_update(
    pool: &PgPool,
    instance_id: &str,
    chat_id: i64,
    message_id: i32,
    claim_token: uuid::Uuid,
) -> anyhow::Result<bool> {
    let result = sqlx::query("update ask_gateway_invocations set lease_until = now() + interval '15 minutes' where instance_id = $1 and chat_id = $2 and message_id = $3 and claim_token = $4 and completed_at is null")
        .bind(instance_id).bind(chat_id).bind(message_id).bind(claim_token).execute(pool).await?;
    Ok(result.rows_affected() == 1)
}

pub async fn release_update(
    pool: &PgPool,
    instance_id: &str,
    chat_id: i64,
    message_id: i32,
    claim_token: uuid::Uuid,
) -> anyhow::Result<()> {
    sqlx::query("delete from ask_gateway_invocations where instance_id = $1 and chat_id = $2 and message_id = $3 and claim_token = $4 and completed_at is null")
        .bind(instance_id).bind(chat_id).bind(message_id).bind(claim_token).execute(pool).await?;
    Ok(())
}

/// Returns the user's text only for an explicitly configured mention or a
/// direct reply to this authenticated bot identity.
pub fn invocation_question(
    message: &Message,
    identity: Option<&TelegramBotIdentity>,
    mention_enabled: bool,
    reply_enabled: bool,
    private_enabled: bool,
) -> Option<String> {
    let sender = message.from.as_ref()?;
    if sender.is_bot
        || identity.is_some_and(|identity| sender.id.0 as i64 == identity.user_id)
        || message.sender_chat.is_some()
        || message.via_bot.is_some()
        || message.forward_origin().is_some()
        || message.is_automatic_forward()
    {
        return None;
    }

    let (text, entities) = if let Some(text) = message.text() {
        (text, message.entities().unwrap_or_default())
    } else {
        let caption = message.caption()?;
        (caption, message.caption_entities().unwrap_or_default())
    };
    if text.trim_start().starts_with('/') {
        return None;
    }

    if message.chat.is_private() {
        return (private_enabled && !text.trim().is_empty()).then(|| text.trim().to_owned());
    }

    let mention_ranges = if mention_enabled {
        identity
            .and_then(|identity| identity.username.as_deref())
            .map(|username| matching_mentions(text, entities, username))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let replied_to_bot = reply_enabled
        && identity.is_some()
        && message
            .reply_to_message()
            .and_then(|reply| reply.from.as_ref())
            .is_some_and(|author| author.id.0 as i64 == identity.unwrap().user_id && author.is_bot);

    if mention_ranges.is_empty() && !replied_to_bot {
        return None;
    }
    let mut question = text.to_owned();
    for range in mention_ranges.into_iter().rev() {
        question.replace_range(range, "");
    }
    let question = question.trim().to_owned();
    if question.is_empty() {
        None
    } else {
        Some(question)
    }
}

fn matching_mentions(
    text: &str,
    entities: &[MessageEntity],
    username: &str,
) -> Vec<std::ops::Range<usize>> {
    let username = format!("@{username}");
    let mut result = Vec::new();
    for entity in entities {
        if !matches!(entity.kind, MessageEntityKind::Mention) {
            continue;
        }
        let Some(range) = utf16_range(text, entity.offset, entity.length) else {
            continue;
        };
        if text[range.clone()].eq_ignore_ascii_case(&username)
            && !entities.iter().any(|quoted| {
                matches!(
                    quoted.kind,
                    MessageEntityKind::Blockquote | MessageEntityKind::ExpandableBlockquote
                ) && ranges_overlap(
                    &range,
                    &utf16_range(text, quoted.offset, quoted.length).unwrap_or(0..0),
                )
            })
        {
            result.push(range);
        }
    }
    result
}

fn ranges_overlap(left: &std::ops::Range<usize>, right: &std::ops::Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

#[cfg(test)]
mod tests {
    use super::{invocation_question, run_while_claimed, utf16_range};
    use crate::state::TelegramBotIdentity;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use teloxide::types::Message;

    fn message(value: serde_json::Value) -> Message {
        serde_json::from_value(value).expect("valid synthetic Telegram message")
    }

    #[tokio::test]
    async fn losing_the_lease_cancels_the_in_flight_execution() {
        struct DropSignal(Arc<AtomicBool>);
        impl Drop for DropSignal {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let was_dropped = Arc::new(AtomicBool::new(false));
        let future_signal = Arc::clone(&was_dropped);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let future = async move {
            let _drop_signal = DropSignal(future_signal);
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        };
        let (lease_lost_tx, lease_lost_rx) = tokio::sync::oneshot::channel();
        let execution = tokio::spawn(run_while_claimed(future, lease_lost_rx));
        started_rx.await.unwrap();
        lease_lost_tx.send(()).unwrap();
        assert!(execution.await.unwrap().is_none());
        assert!(was_dropped.load(Ordering::SeqCst));
    }

    fn identity() -> TelegramBotIdentity {
        TelegramBotIdentity {
            user_id: 99,
            username: Some("Example_Bot".to_owned()),
        }
    }

    #[test]
    fn utf16_offsets_account_for_supplementary_characters() {
        let text = "😀 @Example_Bot help";
        let range = utf16_range(text, 3, 12).unwrap();
        assert_eq!(&text[range], "@Example_Bot");
    }

    #[test]
    fn exact_mention_with_emoji_activates_and_removes_only_entity() {
        let msg = message(serde_json::json!({
            "message_id": 1, "date": 1,
            "chat": {"id": -10, "type": "supergroup", "title": "test"},
            "from": {"id": 7, "is_bot": false, "first_name": "Alice"},
            "text": "😀 @example_bot, помоги!",
            "entities": [{"type": "mention", "offset": 3, "length": 12}]
        }));
        assert_eq!(
            invocation_question(&msg, Some(&identity()), true, true, false).as_deref(),
            Some("😀 , помоги!")
        );
        assert!(invocation_question(&msg, Some(&identity()), false, false, false).is_none());
    }

    #[test]
    fn mention_of_a_different_bot_does_not_activate() {
        let msg = message(serde_json::json!({
            "message_id": 9, "date": 1,
            "chat": {"id": -10, "type": "supergroup", "title": "test"},
            "from": {"id": 7, "is_bot": false, "first_name": "Alice"},
            "text": "@Another_Bot help",
            "entities": [{"type": "mention", "offset": 0, "length": 12}]
        }));
        assert!(invocation_question(&msg, Some(&identity()), true, false, false).is_none());
    }

    #[test]
    fn substrings_unknown_commands_and_unrelated_messages_do_not_activate() {
        let make = |text: &str| {
            message(serde_json::json!({
                "message_id": 2, "date": 1,
                "chat": {"id": -10, "type": "supergroup", "title": "test"},
                "from": {"id": 7, "is_bot": false, "first_name": "Alice"},
                "text": text
            }))
        };
        assert!(
            invocation_question(
                &make("example_bot is mentioned in plain text"),
                Some(&identity()),
                true,
                true,
                false
            )
            .is_none()
        );
        assert!(
            invocation_question(
                &make("/unknown @Example_Bot help"),
                Some(&identity()),
                true,
                true,
                false
            )
            .is_none()
        );
        assert!(
            invocation_question(
                &make("ordinary conversation"),
                Some(&identity()),
                true,
                true,
                false
            )
            .is_none()
        );
    }

    #[test]
    fn direct_reply_requires_the_authenticated_bot_author() {
        let make = |bot_id: i64| {
            message(serde_json::json!({
                "message_id": 4, "date": 1,
                "chat": {"id": -10, "type": "supergroup", "title": "test"},
                "from": {"id": 7, "is_bot": false, "first_name": "Alice"},
                "text": "please explain",
                "reply_to_message": {
                    "message_id": 3, "date": 1,
                    "chat": {"id": -10, "type": "supergroup", "title": "test"},
                    "from": {"id": bot_id, "is_bot": true, "first_name": "Example"},
                    "text": "How can I help?"
                }
            }))
        };
        assert_eq!(
            invocation_question(&make(99), Some(&identity()), false, true, false).as_deref(),
            Some("please explain")
        );
        assert!(invocation_question(&make(100), Some(&identity()), false, true, false).is_none());
    }

    #[test]
    fn quoted_mentions_and_via_bot_messages_are_ignored() {
        let quoted = message(serde_json::json!({
            "message_id": 5, "date": 1,
            "chat": {"id": -10, "type": "supergroup", "title": "test"},
            "from": {"id": 7, "is_bot": false, "first_name": "Alice"},
            "text": "@Example_Bot help",
            "entities": [
                {"type": "mention", "offset": 0, "length": 12},
                {"type": "blockquote", "offset": 0, "length": 17}
            ]
        }));
        assert!(invocation_question(&quoted, Some(&identity()), true, true, false).is_none());
        let via_bot = message(serde_json::json!({
            "message_id": 6, "date": 1,
            "chat": {"id": -10, "type": "supergroup", "title": "test"},
            "from": {"id": 7, "is_bot": false, "first_name": "Alice"},
            "via_bot": {"id": 12, "is_bot": true, "first_name": "Other"},
            "text": "@Example_Bot help",
            "entities": [{"type": "mention", "offset": 0, "length": 12}]
        }));
        assert!(invocation_question(&via_bot, Some(&identity()), true, true, false).is_none());
    }

    #[test]
    fn bot_self_messages_and_forwarded_mentions_are_ignored() {
        let self_message = message(serde_json::json!({
            "message_id": 7, "date": 1,
            "chat": {"id": -10, "type": "supergroup", "title": "test"},
            "from": {"id": 99, "is_bot": true, "first_name": "Example"},
            "text": "@Example_Bot help",
            "entities": [{"type": "mention", "offset": 0, "length": 12}]
        }));
        assert!(invocation_question(&self_message, Some(&identity()), true, true, false).is_none());
        let forwarded = message(serde_json::json!({
            "message_id": 8, "date": 1,
            "chat": {"id": -10, "type": "supergroup", "title": "test"},
            "from": {"id": 7, "is_bot": false, "first_name": "Alice"},
            "forward_origin": {
                "type": "user", "date": 1,
                "sender_user": {"id": 8, "is_bot": false, "first_name": "Forwarded"}
            },
            "text": "@Example_Bot help",
            "entities": [{"type": "mention", "offset": 0, "length": 12}]
        }));
        assert!(invocation_question(&forwarded, Some(&identity()), true, true, false).is_none());
    }

    #[test]
    fn private_direct_messages_are_opt_in_and_unknown_users_are_not_authorized_here() {
        let msg = message(serde_json::json!({
            "message_id": 3, "date": 1,
            "chat": {"id": 7, "type": "private", "first_name": "Alice"},
            "from": {"id": 7, "is_bot": false, "first_name": "Alice"},
            "text": "please help"
        }));
        assert!(invocation_question(&msg, None, false, false, false).is_none());
        assert_eq!(
            invocation_question(&msg, None, false, false, true).as_deref(),
            Some("please help")
        );
    }
}
