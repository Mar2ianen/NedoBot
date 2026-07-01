use std::{collections::HashMap, fs::File, io::BufReader, path::PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{
    PgPool,
    types::chrono::{DateTime, Utc},
};

#[derive(Debug)]
struct Args {
    export_path: PathBuf,
    chat_id: Option<i64>,
    batch_size: usize,
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
struct ExportRoot {
    id: i64,
    name: String,
    #[serde(rename = "type")]
    chat_type: String,
    messages: Vec<ExportMessage>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExportMessage {
    id: i32,
    #[serde(rename = "type")]
    message_type: String,
    date: String,
    date_unixtime: String,
    from: Option<String>,
    from_id: Option<String>,
    actor: Option<String>,
    actor_id: Option<String>,
    reply_to_message_id: Option<i32>,
    text: Value,
    text_entities: Option<Value>,
    photo: Option<String>,
    file: Option<String>,
    media_type: Option<String>,
    mime_type: Option<String>,
    forwarded_from: Option<String>,
    forwarded_from_id: Option<String>,
    via_bot: Option<String>,
    reactions: Option<Vec<ExportReaction>>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExportReaction {
    #[serde(rename = "type")]
    reaction_type: String,
    count: i64,
    emoji: Option<String>,
    document_id: Option<String>,
    recent: Option<Vec<ExportRecentReaction>>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExportRecentReaction {
    from: Option<String>,
    from_id: Option<String>,
    date: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone)]
struct UserProfile {
    user_id: i64,
    first_name: String,
    is_bot: bool,
    last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
struct ImportStats {
    seen: usize,
    imported_messages: usize,
    skipped_messages: usize,
    user_profiles: usize,
    channel_messages: usize,
    reaction_messages: usize,
    reaction_counts: usize,
    recent_reaction_events: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let args = parse_args()?;
    let file = File::open(&args.export_path)
        .with_context(|| format!("failed to open {}", args.export_path.display()))?;
    let root: ExportRoot = serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("failed to parse {}", args.export_path.display()))?;

    let chat_id = args
        .chat_id
        .unwrap_or_else(|| export_chat_id_to_bot_id(root.id));
    let bot_user_id = bot_user_id_from_env();
    let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;

    println!(
        "export: {} ({}, id={}) messages={} -> chat_id={}",
        root.name,
        root.chat_type,
        root.id,
        root.messages.len(),
        chat_id
    );

    let (stats, profiles) = scan_export(&root.messages, bot_user_id);
    println!(
        "scan: seen={} importable={} skipped={} channel_messages={} user_profiles={} reaction_messages={} reaction_counts={} recent_reaction_events={}",
        stats.seen,
        stats.imported_messages,
        stats.skipped_messages,
        stats.channel_messages,
        stats.user_profiles,
        stats.reaction_messages,
        stats.reaction_counts,
        stats.recent_reaction_events
    );

    if args.dry_run {
        println!("dry-run: database was not changed");
        return Ok(());
    }

    let pool = PgPool::connect(&database_url).await?;
    import_messages(&pool, chat_id, &root.messages, args.batch_size).await?;
    import_reactions(&pool, chat_id, &root.messages, args.batch_size).await?;
    upsert_profiles(&pool, profiles).await?;
    rebuild_chat_users(&pool, chat_id).await?;

    println!("import complete");
    Ok(())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut positional = Vec::new();
    let mut chat_id = None;
    let mut batch_size = 1000usize;
    let mut dry_run = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--chat-id" => {
                let value = args.next().context("--chat-id requires value")?;
                chat_id = Some(value.parse().context("invalid --chat-id")?);
            }
            "--batch-size" => {
                let value = args.next().context("--batch-size requires value")?;
                batch_size = value.parse().context("invalid --batch-size")?;
                if batch_size == 0 {
                    bail!("--batch-size must be greater than zero");
                }
            }
            "-h" | "--help" => {
                println!(
                    "Usage: import_telegram_export <result.json> [--chat-id -100...] [--batch-size 1000] [--dry-run]"
                );
                std::process::exit(0);
            }
            _ if arg.starts_with('-') => bail!("unknown option: {arg}"),
            _ => positional.push(PathBuf::from(arg)),
        }
    }

    let Some(export_path) = positional.pop() else {
        bail!("result.json path is required");
    };
    if !positional.is_empty() {
        bail!("only one result.json path is supported");
    }

    Ok(Args {
        export_path,
        chat_id,
        batch_size,
        dry_run,
    })
}

fn scan_export(
    messages: &[ExportMessage],
    bot_user_id: Option<i64>,
) -> (ImportStats, HashMap<i64, UserProfile>) {
    let mut stats = ImportStats::default();
    let mut profiles = HashMap::<i64, UserProfile>::new();

    for message in messages {
        stats.seen += 1;
        if message.message_type != "message" && message.message_type != "service" {
            stats.skipped_messages += 1;
            continue;
        }

        stats.imported_messages += 1;
        if let Some(reactions) = message.reactions.as_ref() {
            stats.reaction_messages += 1;
            stats.reaction_counts += reactions.len();
            stats.recent_reaction_events += reactions
                .iter()
                .map(|reaction| reaction.recent.as_ref().map_or(0, Vec::len))
                .sum::<usize>();
        }

        let Some(timestamp) = message_timestamp(message) else {
            continue;
        };

        if message_source_channel_id(message).is_some() {
            stats.channel_messages += 1;
        }

        let Some(user_id) = message_user_id(message) else {
            continue;
        };

        let first_name = message
            .from
            .as_deref()
            .or(message.actor.as_deref())
            .unwrap_or("пользователь")
            .trim()
            .to_string();

        let is_bot = bot_user_id == Some(user_id);
        profiles
            .entry(user_id)
            .and_modify(|profile| {
                if timestamp > profile.last_seen_at {
                    profile.first_name = first_name.clone();
                    profile.last_seen_at = timestamp;
                }
                profile.is_bot |= is_bot;
            })
            .or_insert(UserProfile {
                user_id,
                first_name,
                is_bot,
                last_seen_at: timestamp,
            });

        for recent in message
            .reactions
            .iter()
            .flatten()
            .filter_map(|reaction| reaction.recent.as_ref())
            .flatten()
        {
            let Some(user_id) = parse_prefixed_id(recent.from_id.as_deref(), "user") else {
                continue;
            };
            let Some(timestamp) = parse_export_datetime(&recent.date) else {
                continue;
            };
            let first_name = recent
                .from
                .as_deref()
                .unwrap_or("пользователь")
                .trim()
                .to_string();

            profiles
                .entry(user_id)
                .and_modify(|profile| {
                    if timestamp > profile.last_seen_at {
                        profile.first_name = first_name.clone();
                        profile.last_seen_at = timestamp;
                    }
                })
                .or_insert(UserProfile {
                    user_id,
                    first_name,
                    is_bot: false,
                    last_seen_at: timestamp,
                });
        }
    }

    stats.user_profiles = profiles.len();
    (stats, profiles)
}

async fn import_messages(
    pool: &PgPool,
    chat_id: i64,
    messages: &[ExportMessage],
    batch_size: usize,
) -> anyhow::Result<()> {
    let mut imported = 0usize;

    for chunk in messages.chunks(batch_size) {
        let mut tx = pool.begin().await?;
        for message in chunk {
            if message.message_type != "message" && message.message_type != "service" {
                continue;
            }

            let Some(created_at) = message_timestamp(message) else {
                continue;
            };

            let source_channel_id = message_source_channel_id(message);
            let user_id = message_user_id(message);
            let text = message_text(&message.text);
            let media_type = message.media_type.as_deref();
            let raw_json = serde_json::to_value(message)?;

            sqlx::query(
                r#"
                insert into telegram_messages
                    (
                        chat_id, message_id, user_id, source_channel_id, source_message_id,
                        is_automatic_forward, text, raw_json, created_at, reply_to_message_id,
                        reply_to_user_id, sender_chat_id, via_bot_id, has_photo, has_video,
                        has_document, has_audio, has_voice, has_sticker, has_animation,
                        has_links
                    )
                values ($1, $2, $3, $4, null, $5, $6, $7, $8, $9, null, $10, null, $11, $12, $13, $14, $15, $16, $17, $18)
                on conflict (chat_id, message_id) do update set
                    user_id = coalesce(telegram_messages.user_id, excluded.user_id),
                    source_channel_id = coalesce(telegram_messages.source_channel_id, excluded.source_channel_id),
                    is_automatic_forward = telegram_messages.is_automatic_forward or excluded.is_automatic_forward,
                    text = coalesce(nullif(telegram_messages.text, ''), excluded.text),
                    created_at = least(telegram_messages.created_at, excluded.created_at),
                    reply_to_message_id = coalesce(telegram_messages.reply_to_message_id, excluded.reply_to_message_id),
                    sender_chat_id = coalesce(telegram_messages.sender_chat_id, excluded.sender_chat_id),
                    has_photo = telegram_messages.has_photo or excluded.has_photo,
                    has_video = telegram_messages.has_video or excluded.has_video,
                    has_document = telegram_messages.has_document or excluded.has_document,
                    has_audio = telegram_messages.has_audio or excluded.has_audio,
                    has_voice = telegram_messages.has_voice or excluded.has_voice,
                    has_sticker = telegram_messages.has_sticker or excluded.has_sticker,
                    has_animation = telegram_messages.has_animation or excluded.has_animation,
                    has_links = telegram_messages.has_links or excluded.has_links
                "#,
            )
            .bind(chat_id)
            .bind(message.id)
            .bind(user_id)
            .bind(source_channel_id)
            .bind(source_channel_id.is_some())
            .bind(text)
            .bind(raw_json)
            .bind(created_at)
            .bind(message.reply_to_message_id)
            .bind(source_channel_id)
            .bind(message.photo.is_some())
            .bind(matches!(media_type, Some("video_file" | "video_message")))
            .bind(message.file.is_some() && media_type.is_none())
            .bind(matches!(media_type, Some("audio_file")))
            .bind(matches!(media_type, Some("voice_message")))
            .bind(matches!(media_type, Some("sticker")))
            .bind(matches!(media_type, Some("animation")))
            .bind(message_has_links(message))
            .execute(&mut *tx)
            .await?;

            imported += 1;
        }
        tx.commit().await?;
        println!("messages imported/upserted: {imported}");
    }

    Ok(())
}

async fn import_reactions(
    pool: &PgPool,
    chat_id: i64,
    messages: &[ExportMessage],
    batch_size: usize,
) -> anyhow::Result<()> {
    let mut count_rows = 0usize;
    let mut event_rows = 0usize;

    for chunk in messages.chunks(batch_size) {
        let mut tx = pool.begin().await?;
        for message in chunk {
            let Some(reactions) = message.reactions.as_ref().filter(|items| !items.is_empty())
            else {
                continue;
            };
            let Some(message_at) = message_timestamp(message) else {
                continue;
            };

            let reactions_json = serde_json::to_value(reactions)?;
            let total_count = reactions
                .iter()
                .map(|reaction| reaction.count.max(0))
                .sum::<i64>();

            sqlx::query(
                r#"
                insert into telegram_message_reaction_counts
                    (chat_id, message_id, reactions, total_count, raw_json, event_at)
                values ($1, $2, $3, $4, $5, $6)
                on conflict (chat_id, message_id) do update set
                    reactions = excluded.reactions,
                    total_count = excluded.total_count,
                    raw_json = excluded.raw_json,
                    event_at = excluded.event_at,
                    updated_at = now()
                "#,
            )
            .bind(chat_id)
            .bind(message.id)
            .bind(&reactions_json)
            .bind(total_count as i32)
            .bind(serde_json::json!({
                "source": "telegram_export",
                "reactions": reactions,
            }))
            .bind(message_at)
            .execute(&mut *tx)
            .await?;
            count_rows += 1;

            for reaction in reactions {
                let new_reactions = serde_json::json!([reaction_identity(reaction)]);
                for recent in reaction.recent.iter().flatten() {
                    let event_at = parse_export_datetime(&recent.date).unwrap_or(message_at);
                    let raw_json = serde_json::json!({
                        "source": "telegram_export_recent",
                        "reaction": reaction,
                        "recent": recent,
                    });

                    sqlx::query(
                        r#"
                        insert into telegram_message_reactions
                            (chat_id, message_id, user_id, actor_chat_id, old_reactions, new_reactions, raw_json, event_at)
                        values ($1, $2, $3, null, '[]'::jsonb, $4, $5, $6)
                        on conflict do nothing
                        "#,
                    )
                    .bind(chat_id)
                    .bind(message.id)
                    .bind(parse_prefixed_id(recent.from_id.as_deref(), "user"))
                    .bind(&new_reactions)
                    .bind(raw_json)
                    .bind(event_at)
                    .execute(&mut *tx)
                    .await?;
                    event_rows += 1;
                }
            }
        }
        tx.commit().await?;
        println!(
            "reaction counts upserted: {count_rows}, recent reaction events seen: {event_rows}"
        );
    }

    Ok(())
}

async fn upsert_profiles(pool: &PgPool, profiles: HashMap<i64, UserProfile>) -> anyhow::Result<()> {
    let mut imported = 0usize;
    let mut tx = pool.begin().await?;

    for profile in profiles.into_values() {
        sqlx::query(
            r#"
            insert into telegram_user_profiles
                (telegram_user_id, username, first_name, last_name, is_bot, last_seen_at, updated_at)
            values ($1, null, $2, null, $3, $4, now())
            on conflict (telegram_user_id) do update set
                first_name = coalesce(nullif(telegram_user_profiles.first_name, ''), excluded.first_name),
                is_bot = telegram_user_profiles.is_bot or excluded.is_bot,
                last_seen_at = greatest(telegram_user_profiles.last_seen_at, excluded.last_seen_at),
                updated_at = now()
            "#,
        )
        .bind(profile.user_id)
        .bind(profile.first_name)
        .bind(profile.is_bot)
        .bind(profile.last_seen_at)
        .execute(&mut *tx)
        .await?;
        imported += 1;
    }

    tx.commit().await?;
    println!("profiles upserted: {imported}");
    Ok(())
}

async fn rebuild_chat_users(pool: &PgPool, chat_id: i64) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        with recursive post_thread_messages as (
            select chat_id, message_id
            from telegram_messages
            where chat_id = $1 and source_channel_id is not null

            union

            select child.chat_id, child.message_id
            from telegram_messages child
            join post_thread_messages parent
              on parent.chat_id = child.chat_id
             and parent.message_id = child.reply_to_message_id
            where child.chat_id = $1
              and child.source_channel_id is null
        )
        insert into telegram_chat_users
            (
                chat_id, telegram_user_id, first_seen_at, last_seen_at,
                first_message_id, last_message_id, message_count, reply_count,
                link_count, media_count, reply_to_channel_post_count, reply_to_bot_count,
                member_status, is_admin, is_present, member_observed_at, updated_at
            )
        select
            m.chat_id,
            m.user_id,
            min(m.created_at) as first_seen_at,
            max(m.created_at) as last_seen_at,
            (array_agg(m.message_id order by m.created_at, m.message_id))[1] as first_message_id,
            (array_agg(m.message_id order by m.created_at desc, m.message_id desc))[1] as last_message_id,
            count(*)::bigint as message_count,
            count(*) filter (where m.reply_to_message_id is not null)::bigint as reply_count,
            count(*) filter (where m.has_links)::bigint as link_count,
            count(*) filter (where m.has_photo or m.has_video or m.has_document or m.has_audio or m.has_voice or m.has_sticker or m.has_animation)::bigint as media_count,
            count(*) filter (where m.message_id in (select message_id from post_thread_messages))::bigint as reply_to_channel_post_count,
            count(*) filter (where m.reply_to_message_id in (select bot_comment_message_id from post_comment_jobs where discussion_chat_id = m.chat_id))::bigint as reply_to_bot_count,
            s.status,
            coalesce(s.is_admin, false),
            s.is_present,
            s.observed_at,
            now()
        from telegram_messages m
        left join telegram_chat_member_snapshots s
            on s.chat_id = m.chat_id
           and s.telegram_user_id = m.user_id
        where m.chat_id = $1
          and m.user_id is not null
          and m.source_channel_id is null
        group by m.chat_id, m.user_id, s.status, s.is_admin, s.is_present, s.observed_at
        on conflict (chat_id, telegram_user_id) do update set
            first_seen_at = excluded.first_seen_at,
            last_seen_at = excluded.last_seen_at,
            first_message_id = excluded.first_message_id,
            last_message_id = excluded.last_message_id,
            message_count = excluded.message_count,
            reply_count = excluded.reply_count,
            link_count = excluded.link_count,
            media_count = excluded.media_count,
            reply_to_channel_post_count = excluded.reply_to_channel_post_count,
            reply_to_bot_count = excluded.reply_to_bot_count,
            member_status = coalesce(excluded.member_status, telegram_chat_users.member_status),
            is_admin = excluded.is_admin,
            is_present = coalesce(excluded.is_present, telegram_chat_users.is_present),
            member_observed_at = coalesce(excluded.member_observed_at, telegram_chat_users.member_observed_at),
            updated_at = now()
        "#,
    )
    .bind(chat_id)
    .execute(pool)
    .await?;

    println!("telegram_chat_users rebuilt for chat_id={chat_id}");
    Ok(())
}

fn export_chat_id_to_bot_id(id: i64) -> i64 {
    if id < 0 {
        id
    } else {
        -(1_000_000_000_000 + id)
    }
}

fn bot_user_id_from_env() -> Option<i64> {
    std::env::var("TELOXIDE_TOKEN")
        .ok()
        .and_then(|token| token.split_once(':').map(|(id, _)| id.to_string()))
        .and_then(|id| id.parse().ok())
}

fn message_timestamp(message: &ExportMessage) -> Option<DateTime<Utc>> {
    message
        .date_unixtime
        .parse::<i64>()
        .ok()
        .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
}

fn parse_export_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|date| date.to_utc())
        .ok()
        .or_else(|| {
            let value = format!("{value}+00:00");
            DateTime::parse_from_rfc3339(&value)
                .map(|date| date.to_utc())
                .ok()
        })
}

fn message_user_id(message: &ExportMessage) -> Option<i64> {
    parse_prefixed_id(message.from_id.as_deref(), "user")
        .or_else(|| parse_prefixed_id(message.actor_id.as_deref(), "user"))
}

fn message_source_channel_id(message: &ExportMessage) -> Option<i64> {
    parse_prefixed_id(message.from_id.as_deref(), "channel")
        .or_else(|| parse_prefixed_id(message.forwarded_from_id.as_deref(), "channel"))
}

fn parse_prefixed_id(value: Option<&str>, prefix: &str) -> Option<i64> {
    let id = value?.strip_prefix(prefix)?.parse::<i64>().ok()?;
    Some(if prefix == "channel" {
        export_chat_id_to_bot_id(id)
    } else {
        id
    })
}

fn message_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| match part {
                Value::String(text) => Some(text.as_str()),
                Value::Object(object) => object.get("text").and_then(Value::as_str),
                _ => None,
            })
            .collect::<String>(),
        _ => String::new(),
    }
}

fn message_has_links(message: &ExportMessage) -> bool {
    let text = message_text(&message.text);
    if text.contains("http://") || text.contains("https://") || text.contains("t.me/") {
        return true;
    }

    value_has_link_entity(&message.text)
        || message
            .text_entities
            .as_ref()
            .is_some_and(value_has_link_entity)
}

fn value_has_link_entity(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(value_has_link_entity),
        Value::Object(object) => object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| matches!(kind, "link" | "text_link" | "url")),
        _ => false,
    }
}

fn reaction_identity(reaction: &ExportReaction) -> Value {
    let mut object = serde_json::Map::new();
    object.insert(
        "type".to_string(),
        Value::String(reaction.reaction_type.clone()),
    );
    if let Some(emoji) = reaction.emoji.as_ref() {
        object.insert("emoji".to_string(), Value::String(emoji.clone()));
    }
    if let Some(document_id) = reaction.document_id.as_ref() {
        object.insert(
            "document_id".to_string(),
            Value::String(document_id.clone()),
        );
    }
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_export_chat_id_to_bot_api_id() {
        assert_eq!(export_chat_id_to_bot_id(1932061163), -1001932061163);
    }

    #[test]
    fn flattens_rich_text() {
        let text = serde_json::json!(["hello ", {"type": "link", "text": "https://t.me/x"}]);
        assert_eq!(message_text(&text), "hello https://t.me/x");
    }
}
