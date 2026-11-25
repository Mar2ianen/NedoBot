//! Topic context из проверенного исходного channel post, без соседних реплик.
use crate::{config::Config, features::new_user_audit::local_cascade::hash_bytes};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgConnection, PgPool};

const MAX_ANCESTORS: i32 = 16;
const MAX_TOPIC_CHARS: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PostSnapshot {
    pub message_id: i32,
    pub source_channel_id: i64,
    pub source_hash: String,
}

#[derive(FromRow)]
struct Post {
    message_id: i32,
    source_channel_id: i64,
    text: String,
}

pub(crate) fn source_channels(config: &Config, chat: i64) -> Vec<i64> {
    let mut ids: Vec<_> = config
        .first_comment_route_for_chat(chat)
        .map(|r| r.source_channel_id)
        .collect();
    if chat == config.discussion_chat_id && config.source_channel_id != 0 {
        ids.push(config.source_channel_id);
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

pub(crate) async fn load_post_topic(
    pool: &PgPool,
    config: &Config,
    chat: i64,
    message: i32,
) -> anyhow::Result<Option<(String, PostSnapshot)>> {
    let channels = source_channels(config, chat);
    if channels.is_empty() {
        return Ok(None);
    }
    let post:Option<Post>=sqlx::query_as(r#"
        with recursive ancestors as (
            select message_id,reply_to_message_id,source_channel_id,is_automatic_forward,text,deleted_by_bot_at,0 as depth
            from telegram_messages where chat_id=$1 and message_id=$2
            union all
            select p.message_id,p.reply_to_message_id,p.source_channel_id,p.is_automatic_forward,p.text,p.deleted_by_bot_at,c.depth+1
            from telegram_messages p join ancestors c on p.chat_id=$1 and p.message_id=c.reply_to_message_id
            where c.depth<$4 and p.message_id<c.message_id
        )
        select message_id,source_channel_id,coalesce(text,'') as text from ancestors
        where depth>0 and source_channel_id=any($3) and is_automatic_forward and deleted_by_bot_at is null
        order by depth limit 1"#)
        .bind(chat).bind(message).bind(channels).bind(MAX_ANCESTORS).fetch_optional(pool).await?;
    Ok(post.and_then(|p| {
        let override_topic = config
            .community
            .moderation
            .decision
            .post_topics
            .iter()
            .find(|o| o.discussion_chat_id == chat && o.post_message_id == p.message_id);
        let topic = crate::features::new_user_analysis::bounded_audit_text_with_limit(
            Some(override_topic.map_or(p.text.as_str(), |o| o.topic.as_str())),
            MAX_TOPIC_CHARS,
        )?;
        Some((
            topic,
            PostSnapshot {
                message_id: p.message_id,
                source_channel_id: p.source_channel_id,
                source_hash: hash_bytes(p.text.as_bytes()),
            },
        ))
    }))
}

/// Использует уже занятую connection, включая односоединительные captcha pools.
pub(crate) async fn snapshot_is_current(
    connection: &mut PgConnection,
    chat: i64,
    input: &Value,
) -> anyhow::Result<bool> {
    let Some(value) = input.get("_post_context") else {
        return Ok(true);
    };
    if value.is_null() {
        return Ok(true);
    }
    let Ok(snapshot) = serde_json::from_value::<PostSnapshot>(value.clone()) else {
        return Ok(false);
    };
    let text:Option<String>=sqlx::query_scalar("select coalesce(text,'') from telegram_messages where chat_id=$1 and message_id=$2 and source_channel_id=$3 and is_automatic_forward and deleted_by_bot_at is null for share")
        .bind(chat).bind(snapshot.message_id).bind(snapshot.source_channel_id).fetch_optional(connection).await?;
    Ok(text.is_some_and(|text| hash_bytes(text.as_bytes()) == snapshot.source_hash))
}
