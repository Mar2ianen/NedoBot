//! Bounded, observe-only conversation features over the existing message store.
//!
//! This module intentionally does not emit a moderation decision or a spam
//! training label. The snapshot is marked incomplete because the current
//! message table cannot prove that every Telegram update was ingested.

use chrono::{Duration, Utc};
use sqlx::{FromRow, PgPool};
use teloxide::types::{Message, MessageEntityKind};
use teloxide_antispam::{
    flood::{
        ConversationMessage, FloodActor, FloodAssessment, FloodMetricKind, MAX_WINDOW_MESSAGES,
        analyze_window,
    },
    preprocess::{prepare_text, word_sequence},
};

use crate::telegram::entities::{custom_emoji_ids, deserialize_stored_message, message_text};

const WINDOW: Duration = Duration::minutes(15);
const WINDOW_MICROS: u64 = 15 * 60 * 1_000_000;

#[derive(FromRow)]
struct StoredMessage {
    message_id: i32,
    raw_json: serde_json::Value,
}

/// Rebuilds one bounded chat/topic window from the canonical Telegram rows.
/// Edits therefore replace a message's current projection and do not add a
/// second send. `FLOOD_SHADOW_ENABLED` and the Cargo `flood` feature must both
/// be enabled before callers invoke this function.
pub async fn observe_chat(
    pool: &PgPool,
    chat_id: i64,
    thread_id: Option<i32>,
) -> anyhow::Result<FloodAssessment> {
    let now = Utc::now();
    let rows = sqlx::query_as::<_, StoredMessage>(
        r#"
        select message_id, raw_json
        from telegram_messages
        where chat_id = $1
          and created_at >= $3
          and coalesce(raw_json->>'message_thread_id', '0') = $2::text
          and deleted_by_bot_at is null
        order by created_at desc, message_id desc
        limit $4
        "#,
    )
    .bind(chat_id)
    .bind(thread_id.unwrap_or(0))
    .bind(now - WINDOW)
    .bind((MAX_WINDOW_MESSAGES + 1) as i64)
    .fetch_all(pool)
    .await?;

    let complete = false;
    let mut rows = rows;
    if rows.len() > MAX_WINDOW_MESSAGES {
        rows.truncate(MAX_WINDOW_MESSAGES);
    }
    rows.reverse();

    let mut messages = Vec::with_capacity(rows.len());
    for row in rows {
        let message = deserialize_stored_message(row.raw_json)?;
        let event_at = u64::try_from(message.date.timestamp_micros())
            .map_err(|_| anyhow::anyhow!("stored message timestamp predates Unix epoch"))?;
        if let Some(projected) = project_message(&message, row.message_id, event_at) {
            messages.push(projected);
        }
    }

    let now_micros = u64::try_from(now.timestamp_micros())
        .map_err(|_| anyhow::anyhow!("current timestamp predates Unix epoch"))?;
    let assessment = analyze_window(&messages, now_micros, WINDOW_MICROS, complete)?;
    let metric = |kind| assessment.metric(kind).and_then(|metric| metric.value);
    tracing::trace!(
        chat_id,
        thread_id = thread_id.unwrap_or(0),
        logical_messages = metric(FloodMetricKind::LogicalMessages),
        largest_actor_share = metric(FloodMetricKind::LargestActorShare),
        consecutive_actor_run = metric(FloodMetricKind::ConsecutiveActorRun),
        exact_duplicate_pairs = metric(FloodMetricKind::ExactDuplicatePairs),
        maximum_trigram_jaccard = metric(FloodMetricKind::MaximumTrigramJaccard),
        complete = assessment.complete,
        "flood shadow observation"
    );
    Ok(assessment)
}

fn project_message(
    message: &Message,
    message_id: i32,
    event_at: u64,
) -> Option<ConversationMessage> {
    let actor = if let Some(chat) = &message.sender_chat {
        FloodActor::SenderChat(chat.id.0)
    } else {
        let user = message.from.as_ref()?;
        if user.is_bot {
            return None;
        }
        FloodActor::User(user.id.0 as i64)
    };
    let text = message_text(message).unwrap_or_default();
    let prepared = prepare_text(&text);
    // Keep source word order here. The flood core independently derives
    // trigrams; classifier `word_tokens` intentionally includes bigrams.
    let tokens = word_sequence(&prepared.canonical);
    let entities = message
        .entities()
        .into_iter()
        .flatten()
        .chain(message.caption_entities().into_iter().flatten());
    let mention_count = entities
        .filter(|entity| {
            matches!(
                entity.kind,
                MessageEntityKind::Mention | MessageEntityKind::TextMention { .. }
            )
        })
        .count()
        .min(u32::MAX as usize) as u32;
    let media_items = [
        message.photo().is_some(),
        message.video().is_some(),
        message.document().is_some(),
        message.audio().is_some(),
        message.voice().is_some(),
        message.sticker().is_some(),
        message.animation().is_some(),
        message.video_note().is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count()
    .min(u32::MAX as usize) as u32;

    Some(ConversationMessage {
        message_id: i64::from(message_id),
        event_at,
        actor,
        reply_to: message
            .reply_to_message()
            .map(|reply| i64::from(reply.id.0)),
        album_id: message.media_group_id().map(ToString::to_string),
        tokens,
        media_items,
        // The Telegram adapter currently counts custom emoji entities; native
        // Unicode emoji classification belongs to the shared emoji extraction
        // work and is not approximated here.
        emoji_count: custom_emoji_ids(message).len().min(u32::MAX as usize) as u32,
        mention_count,
        forwarded: message.forward_origin().is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::project_message;
    use chrono::Utc;
    use sqlx::PgPool;
    use teloxide::types::Message;
    use teloxide_antispam::flood::{FloodActor, FloodMetricKind, analyze_window};

    #[test]
    fn projects_album_items_as_separate_media_but_one_logical_send() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 17,
            "date": 1700000000,
            "media_group_id": "album-1",
            "chat": {"id": -1002, "type": "supergroup", "title": "Test"},
            "from": {"id": 42, "is_bot": false, "first_name": "A"},
            "photo": [{"file_id": "small", "file_unique_id": "s", "width": 10, "height": 10}],
            "caption": "Same useful caption"
        }))
        .expect("synthetic Telegram photo message should deserialize");

        let projected = project_message(&message, 17, 1_700_000_000_000_000).unwrap();
        assert_eq!(projected.actor, FloodActor::User(42));
        assert_eq!(projected.album_id.as_deref(), Some("album-1"));
        assert_eq!(projected.media_items, 1);
        assert!(!projected.tokens.is_empty());
        let assessment =
            analyze_window(&[projected], 1_700_000_000_000_000, 900_000_000, true).unwrap();
        assert_eq!(
            assessment
                .metric(FloodMetricKind::LogicalMessages)
                .and_then(|metric| metric.value),
            Some(1.0)
        );
        assert_eq!(
            assessment
                .metric(FloodMetricKind::MediaItems)
                .and_then(|metric| metric.value),
            Some(1.0)
        );
    }

    #[test]
    fn messages_without_a_stable_author_are_not_projected() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 17,
            "date": 1700000000,
            "chat": {"id": -1002, "type": "channel", "title": "Test"},
            "text": "service"
        }))
        .expect("synthetic channel message should deserialize");
        assert!(project_message(&message, 17, 1).is_none());
    }

    #[test]
    fn bot_messages_are_not_counted_as_conversation_actors() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 18,
            "date": 1700000000,
            "chat": {"id": -1002, "type": "supergroup", "title": "Test"},
            "from": {"id": 43, "is_bot": true, "first_name": "Bot"},
            "text": "automated reply"
        }))
        .expect("synthetic Telegram bot message should deserialize");
        assert!(project_message(&message, 18, 1).is_none());
    }

    #[test]
    fn sender_chat_identity_wins_over_legacy_from_user() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 19,
            "date": 1700000000,
            "chat": {"id": -1002, "type": "supergroup", "title": "Test"},
            "from": {"id": 43, "is_bot": true, "first_name": "Compatibility sender"},
            "sender_chat": {"id": -1003, "type": "channel", "title": "Source"},
            "text": "channel-authored message"
        }))
        .expect("synthetic sender-chat message should deserialize");

        let projected = project_message(&message, 19, 1).unwrap();
        assert_eq!(projected.actor, FloodActor::SenderChat(-1003));
    }

    #[test]
    fn flood_projection_keeps_ordered_words_without_classifier_bigrams() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 20,
            "date": 1700000000,
            "chat": {"id": -1002, "type": "supergroup", "title": "Test"},
            "from": {"id": 42, "is_bot": false, "first_name": "A"},
            "text": "a b c"
        }))
        .expect("synthetic message should deserialize");

        let projected = project_message(&message, 20, 1).unwrap();
        assert_eq!(projected.tokens, ["a", "b", "c"]);
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL and a migrated disposable PostgreSQL database"]
    async fn observer_reads_only_the_requested_chat_and_topic() {
        let pool = PgPool::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let chat_id = -100_000_000_000_i64 - (uuid::Uuid::new_v4().as_u128() % 1_000_000) as i64;
        let now = Utc::now();
        for (message_id, thread_id, text) in [
            (11_i32, 77_i32, "topic message one"),
            (12_i32, 77_i32, "topic message two"),
            (13_i32, 78_i32, "other topic"),
        ] {
            let raw_json = serde_json::json!({
                "message_id": message_id,
                "date": now.timestamp(),
                "message_thread_id": thread_id,
                "chat": {"id": chat_id, "type": "supergroup", "title": "Flood test"},
                "from": {"id": 42, "is_bot": false, "first_name": "A"},
                "text": text
            });
            sqlx::query(
                "insert into telegram_messages (chat_id, message_id, text, raw_json, created_at) values ($1, $2, $3, $4, $5)",
            )
            .bind(chat_id)
            .bind(message_id)
            .bind(text)
            .bind(raw_json)
            .bind(now - chrono::Duration::seconds(i64::from(message_id - 10)))
            .execute(&pool)
            .await
            .unwrap();
        }

        let assessment = super::observe_chat(&pool, chat_id, Some(77)).await.unwrap();
        assert_eq!(
            assessment
                .metric(FloodMetricKind::LogicalMessages)
                .and_then(|metric| metric.value),
            Some(2.0)
        );
        assert!(!assessment.complete);

        sqlx::query("delete from telegram_messages where chat_id = $1")
            .bind(chat_id)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }
}
