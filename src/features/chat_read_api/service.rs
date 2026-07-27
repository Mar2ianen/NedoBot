use serde::{Deserialize, Serialize};
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

const MAX_QUERY_CHARS: usize = 240;
const MAX_RESULT_LIMIT: i64 = 20;
const MAX_CONTEXT_MESSAGES: i64 = 5;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MessageMatch {
    FullText,
    Literal,
}

impl MessageMatch {
    fn as_str(&self) -> &'static str {
        match self {
            Self::FullText => "full_text",
            Self::Literal => "literal",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MessageSort {
    Relevance,
    Newest,
    Oldest,
}

impl MessageSort {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Newest => "newest",
            Self::Oldest => "oldest",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MessageSearchRequest {
    pub chat_id: i64,
    pub query: String,
    pub user_id: Option<i64>,
    pub date_from: Option<DateTime<Utc>>,
    pub date_to: Option<DateTime<Utc>>,
    pub reply_to_message_id: Option<i32>,
    pub has_links: Option<bool>,
    pub has_media: Option<bool>,
    pub match_mode: MessageMatch,
    pub sort: MessageSort,
    pub limit: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ChatMessage {
    pub message_id: i32,
    pub user_id: Option<i64>,
    pub author: String,
    pub author_url: Option<String>,
    pub text: String,
    pub reply_to_message_id: Option<i32>,
    pub created_at: String,
    pub relevance: i32,
    pub source_id: String,
    pub message_url: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ChatInteraction {
    pub message: ChatMessage,
    pub replied_to: Option<ChatMessage>,
}

#[derive(Clone, Debug)]
pub struct RecentMessagesRequest {
    pub chat_id: i64,
    pub user_id: Option<i64>,
    pub date_from: Option<DateTime<Utc>>,
    pub date_to: Option<DateTime<Utc>>,
    pub has_links: Option<bool>,
    pub has_media: Option<bool>,
    pub sort: MessageSort,
    pub limit: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, FromRow)]
pub struct ChatUserProfile {
    pub telegram_user_id: i64,
    pub username: Option<String>,
    pub display_name: String,
    pub author_url: Option<String>,
    pub bio: Option<String>,
    pub is_bot: bool,
    pub is_premium: Option<bool>,
    pub language_code: Option<String>,
    pub message_count: i64,
    pub message_rank: i64,
    pub reply_count: i64,
    pub link_count: i64,
    pub media_count: i64,
    pub first_seen_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub member_status: Option<String>,
    pub is_admin: bool,
    pub admin_title: Option<String>,
    pub is_present: Option<bool>,
}

#[derive(FromRow)]
struct MessageRow {
    message_id: i32,
    user_id: Option<i64>,
    author: String,
    author_username: Option<String>,
    text: String,
    reply_to_message_id: Option<i32>,
    created_at: DateTime<Utc>,
    relevance: f32,
}

#[derive(FromRow)]
struct InteractionRow {
    message_id: i32,
    user_id: Option<i64>,
    author: String,
    author_username: Option<String>,
    text: String,
    reply_to_message_id: Option<i32>,
    created_at: DateTime<Utc>,
    replied_to_message_id: Option<i32>,
    replied_to_user_id: Option<i64>,
    replied_to_author: Option<String>,
    replied_to_username: Option<String>,
    replied_to_text: Option<String>,
    replied_to_created_at: Option<DateTime<Utc>>,
}

pub async fn search_messages(
    pool: &PgPool,
    request: &MessageSearchRequest,
) -> anyhow::Result<Vec<ChatMessage>> {
    let query = normalized_query(&request.query)?;
    let rows = sqlx::query_as::<_, MessageRow>(
        r#"
        select
            m.message_id,
            m.user_id,
            coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''),
                     nullif(p.username, ''),
                     'Неизвестный пользователь') as author,
            nullif(p.username, '') as author_username,
            m.text,
            m.reply_to_message_id,
            m.created_at,
            case when $11 = 'full_text' then greatest(
                    ts_rank_cd(to_tsvector('russian', coalesce(m.text, '')), websearch_to_tsquery('russian', $2)),
                    ts_rank_cd(to_tsvector('simple', coalesce(m.text, '')), websearch_to_tsquery('simple', $2))
                 ) else case when position(lower($2) in lower(coalesce(m.text, ''))) > 0 then 1::real else 0::real end
            end as relevance
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1
          and m.text is not null
          and m.deleted_by_bot_at is null
          and m.spam_marked_at is null
          and (($11 = 'full_text' and (
                   to_tsvector('russian', coalesce(m.text, '')) @@ websearch_to_tsquery('russian', $2)
                or to_tsvector('simple', coalesce(m.text, '')) @@ websearch_to_tsquery('simple', $2)
              )) or ($11 = 'literal' and position(lower($2) in lower(coalesce(m.text, ''))) > 0))
          and ($3::bigint is null or m.user_id = $3)
          and ($4::timestamptz is null or m.created_at >= $4)
          and ($5::timestamptz is null or m.created_at <= $5)
          and ($6::integer is null or m.reply_to_message_id = $6)
          and ($7::boolean is null or m.has_links = $7)
          and (
              $8::boolean is null
              or (m.has_photo or m.has_video or m.has_document or m.has_audio
                  or m.has_voice or m.has_sticker or m.has_animation) = $8
          )
        order by
            case when $9 = 'newest' then m.created_at end desc,
            case when $9 = 'oldest' then m.created_at end asc,
            relevance desc,
            m.created_at desc,
            m.message_id desc
        limit $10
        "#,
    )
    .bind(request.chat_id)
    .bind(&query)
    .bind(request.user_id)
    .bind(request.date_from)
    .bind(request.date_to)
    .bind(request.reply_to_message_id)
    .bind(request.has_links)
    .bind(request.has_media)
    .bind(request.sort.as_str())
    .bind(request.limit.clamp(1, MAX_RESULT_LIMIT))
    .bind(request.match_mode.as_str())
    .fetch_all(pool)
    .await?;

    Ok(map_rows(request.chat_id, rows))
}

pub async fn recent_messages(
    pool: &PgPool,
    request: &RecentMessagesRequest,
) -> anyhow::Result<Vec<ChatMessage>> {
    let rows = sqlx::query_as::<_, MessageRow>(
        r#"
        select m.message_id, m.user_id,
               coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''), nullif(p.username, ''), 'Неизвестный пользователь') as author,
               nullif(p.username, '') as author_username,
               coalesce(m.text, '[медиа без текста]') as text,
               m.reply_to_message_id, m.created_at, 0::real as relevance
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1
          and m.deleted_by_bot_at is null
          and m.spam_marked_at is null
          and ($2::bigint is null or m.user_id = $2)
          and ($3::timestamptz is null or m.created_at >= $3)
          and ($4::timestamptz is null or m.created_at <= $4)
          and ($5::boolean is null or m.has_links = $5)
          and ($6::boolean is null or (m.has_photo or m.has_video or m.has_document or m.has_audio or m.has_voice or m.has_sticker or m.has_animation) = $6)
        order by
            case when $7 = 'oldest' then m.created_at end asc,
            case when $7 <> 'oldest' then m.created_at end desc,
            case when $7 = 'oldest' then m.message_id end asc,
            m.message_id desc
        limit $8
        "#,
    )
    .bind(request.chat_id)
    .bind(request.user_id)
    .bind(request.date_from)
    .bind(request.date_to)
    .bind(request.has_links)
    .bind(request.has_media)
    .bind(request.sort.as_str())
    .bind(request.limit.clamp(1, MAX_RESULT_LIMIT))
    .fetch_all(pool)
    .await?;
    Ok(map_rows(request.chat_id, rows))
}

pub async fn message_context(
    pool: &PgPool,
    chat_id: i64,
    message_id: i32,
    before: i64,
    after: i64,
) -> anyhow::Result<Vec<ChatMessage>> {
    let rows = sqlx::query_as::<_, MessageRow>(
        r#"
        select
            m.message_id,
            m.user_id,
            coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''),
                     nullif(p.username, ''),
                     'Неизвестный пользователь') as author,
            nullif(p.username, '') as author_username,
            coalesce(m.text, '[медиа без текста]') as text,
            m.reply_to_message_id,
            m.created_at,
            0::real as relevance
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1
          and m.deleted_by_bot_at is null
          and m.spam_marked_at is null
          and m.message_id between $2 - $3 and $2 + $4
        order by m.message_id asc
        "#,
    )
    .bind(chat_id)
    .bind(message_id)
    .bind(before.clamp(0, MAX_CONTEXT_MESSAGES) as i32)
    .bind(after.clamp(0, MAX_CONTEXT_MESSAGES) as i32)
    .fetch_all(pool)
    .await?;

    Ok(map_rows(chat_id, rows))
}

pub async fn reply_thread(
    pool: &PgPool,
    chat_id: i64,
    message_id: i32,
) -> anyhow::Result<Vec<ChatMessage>> {
    let rows = sqlx::query_as::<_, MessageRow>(r#"
        with recursive ancestors as (
            select m.message_id, m.user_id, m.text, m.reply_to_message_id, m.created_at, 0 as depth
            from telegram_messages m
            where m.chat_id = $1 and m.message_id = $2 and m.deleted_by_bot_at is null and m.spam_marked_at is null
            union all
            select parent.message_id, parent.user_id, parent.text, parent.reply_to_message_id, parent.created_at, ancestors.depth + 1
            from telegram_messages parent join ancestors on ancestors.reply_to_message_id = parent.message_id
            where parent.chat_id = $1 and ancestors.depth < 5 and parent.deleted_by_bot_at is null and parent.spam_marked_at is null
        ), descendants as (
            select m.message_id, m.user_id, m.text, m.reply_to_message_id, m.created_at, 0 as depth
            from telegram_messages m
            where m.chat_id = $1 and m.message_id = $2 and m.deleted_by_bot_at is null and m.spam_marked_at is null
            union all
            select child.message_id, child.user_id, child.text, child.reply_to_message_id, child.created_at, descendants.depth + 1
            from descendants
            join lateral (
                select candidate.message_id, candidate.user_id, candidate.text, candidate.reply_to_message_id, candidate.created_at
                from telegram_messages candidate
                where candidate.chat_id = $1
                  and candidate.reply_to_message_id = descendants.message_id
                  and candidate.deleted_by_bot_at is null
                  and candidate.spam_marked_at is null
                order by candidate.created_at asc, candidate.message_id asc
                limit 5
            ) child on true
            where descendants.depth < 3
        ), thread as (
            select message_id, user_id, text, reply_to_message_id, created_at from ancestors
            union
            select message_id, user_id, text, reply_to_message_id, created_at from descendants
        )
        select thread.message_id, thread.user_id, coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''), nullif(p.username, ''), 'Неизвестный пользователь') as author,
               nullif(p.username, '') as author_username,
               coalesce(thread.text, '[медиа без текста]') as text, thread.reply_to_message_id, thread.created_at, 0::real as relevance
        from thread left join telegram_user_profiles p on p.telegram_user_id = thread.user_id
        order by thread.created_at asc, thread.message_id asc
        limit 20
    "#).bind(chat_id).bind(message_id).fetch_all(pool).await?;
    Ok(map_rows(chat_id, rows))
}

pub async fn user_interactions(
    pool: &PgPool,
    chat_id: i64,
    first_user_id: i64,
    second_user_id: i64,
    limit: i64,
) -> anyhow::Result<Vec<ChatInteraction>> {
    let rows = sqlx::query_as::<_, InteractionRow>(
        r#"
        select m.message_id, m.user_id,
               coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''),
                        nullif(p.username, ''), 'Неизвестный пользователь') as author,
               nullif(p.username, '') as author_username,
               coalesce(m.text, '[медиа без текста]') as text,
               m.reply_to_message_id, m.created_at,
               replied.message_id as replied_to_message_id,
               replied.user_id as replied_to_user_id,
               coalesce(nullif(concat_ws(' ', replied_profile.first_name, replied_profile.last_name), ''),
                        nullif(replied_profile.username, ''), 'Неизвестный пользователь') as replied_to_author,
               nullif(replied_profile.username, '') as replied_to_username,
               coalesce(replied.text, '[медиа без текста]') as replied_to_text,
               replied.created_at as replied_to_created_at
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_messages replied
          on replied.chat_id = m.chat_id and replied.message_id = m.reply_to_message_id
        left join telegram_user_profiles replied_profile on replied_profile.telegram_user_id = replied.user_id
        where m.chat_id = $1
          and m.deleted_by_bot_at is null
          and m.spam_marked_at is null
          and ((m.user_id = $2 and m.reply_to_user_id = $3)
            or (m.user_id = $3 and m.reply_to_user_id = $2))
        order by m.created_at desc, m.message_id desc
        limit $4
        "#,
    )
    .bind(chat_id)
    .bind(first_user_id)
    .bind(second_user_id)
    .bind(limit.clamp(1, MAX_RESULT_LIMIT))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let message = ChatMessage {
                source_id: source_id(row.message_id),
                message_url: message_url(chat_id, row.message_id),
                relevance: 0,
                message_id: row.message_id,
                user_id: row.user_id,
                author: row.author,
                author_url: author_url(row.author_username.as_deref()),
                text: first_chars(&row.text, 700),
                reply_to_message_id: row.reply_to_message_id,
                created_at: row.created_at.to_rfc3339(),
            };
            let replied_to = row.replied_to_message_id.map(|message_id| ChatMessage {
                source_id: source_id(message_id),
                message_url: message_url(chat_id, message_id),
                relevance: 0,
                message_id,
                user_id: row.replied_to_user_id,
                author: row
                    .replied_to_author
                    .unwrap_or_else(|| "Неизвестный пользователь".to_string()),
                author_url: author_url(row.replied_to_username.as_deref()),
                text: first_chars(
                    row.replied_to_text
                        .as_deref()
                        .unwrap_or("[медиа без текста]"),
                    700,
                ),
                reply_to_message_id: None,
                created_at: row
                    .replied_to_created_at
                    .map(|value| value.to_rfc3339())
                    .unwrap_or_default(),
            });
            ChatInteraction {
                message,
                replied_to,
            }
        })
        .collect())
}

pub async fn user_profile(
    pool: &PgPool,
    chat_id: i64,
    telegram_user_id: i64,
) -> anyhow::Result<Option<ChatUserProfile>> {
    let mut profile = sqlx::query_as::<_, ChatUserProfile>(
        r#"
        select p.telegram_user_id, nullif(p.username, '') as username,
               coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''), nullif(p.username, ''), 'Неизвестный пользователь') as display_name,
               null::text as author_url, nullif(p.bio, '') as bio, p.is_bot, p.is_premium, p.language_code,
               coalesce(cu.message_count, 0) as message_count, coalesce(cu.reply_count, 0) as reply_count,
               1 + (
                   select count(*)
                   from telegram_chat_users ranked
                   left join telegram_user_profiles ranked_profile
                     on ranked_profile.telegram_user_id = ranked.telegram_user_id
                   where ranked.chat_id = $1
                     and not coalesce(ranked_profile.is_bot, false)
                     and ranked.message_count > coalesce(cu.message_count, 0)
               ) as message_rank,
               coalesce(cu.link_count, 0) as link_count, coalesce(cu.media_count, 0) as media_count,
               cu.first_seen_at::text as first_seen_at, cu.last_seen_at::text as last_seen_at,
               cu.member_status, coalesce(cu.is_admin, false) as is_admin,
               nullif(member_snapshot.raw_json ->> 'custom_title', '') as admin_title,
               cu.is_present
        from telegram_user_profiles p
        left join telegram_chat_users cu on cu.chat_id = $1 and cu.telegram_user_id = p.telegram_user_id
        left join telegram_chat_member_snapshots member_snapshot
          on member_snapshot.chat_id = $1 and member_snapshot.telegram_user_id = p.telegram_user_id
        where p.telegram_user_id = $2
          and exists (select 1 from telegram_messages m where m.chat_id = $1 and m.user_id = p.telegram_user_id)
        "#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .fetch_optional(pool)
    .await?;
    if let Some(profile) = profile.as_mut() {
        profile.author_url = author_url(profile.username.as_deref());
    }
    Ok(profile)
}

fn map_rows(chat_id: i64, rows: Vec<MessageRow>) -> Vec<ChatMessage> {
    rows.into_iter()
        .map(|row| ChatMessage {
            source_id: source_id(row.message_id),
            message_url: message_url(chat_id, row.message_id),
            relevance: (row.relevance * 1000.0).round() as i32,
            message_id: row.message_id,
            user_id: row.user_id,
            author: row.author,
            author_url: author_url(row.author_username.as_deref()),
            text: first_chars(&row.text, 700),
            reply_to_message_id: row.reply_to_message_id,
            created_at: row.created_at.to_rfc3339(),
        })
        .collect()
}

pub fn source_id(message_id: i32) -> String {
    format!("chat:{message_id}")
}

pub fn message_url(chat_id: i64, message_id: i32) -> Option<String> {
    let internal_id = chat_id.to_string().strip_prefix("-100")?.to_string();
    Some(format!("https://t.me/c/{internal_id}/{message_id}"))
}

fn author_url(username: Option<&str>) -> Option<String> {
    let username = username?.trim();
    let valid = (5..=32).contains(&username.len())
        && username
            .bytes()
            .all(|character| character.is_ascii_alphanumeric() || character == b'_');
    valid.then(|| format!("https://t.me/{username}"))
}

fn normalized_query(query: &str) -> anyhow::Result<String> {
    let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
    if query.is_empty() {
        anyhow::bail!("message search query must not be empty");
    }
    Ok(first_chars(&query, MAX_QUERY_CHARS))
}

fn first_chars(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        let visible_limit = limit.saturating_sub(1);
        let visible: String = truncated.chars().take(visible_limit).collect();
        format!("{visible}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_url_accepts_supergroup_id() {
        assert_eq!(
            message_url(-1001932061163, 42).as_deref(),
            Some("https://t.me/c/1932061163/42")
        );
    }

    #[test]
    fn message_url_rejects_non_supergroup_id() {
        assert_eq!(message_url(-12345, 42), None);
    }

    #[test]
    fn author_url_accepts_telegram_username() {
        assert_eq!(
            author_url(Some("pasha_3060")),
            Some("https://t.me/pasha_3060".to_string())
        );
    }

    #[test]
    fn author_url_rejects_unsafe_username() {
        assert_eq!(author_url(Some("pasha/3060")), None);
    }

    #[test]
    fn normalizes_and_limits_query() {
        assert_eq!(normalized_query("  Rust   MCP ").unwrap(), "Rust MCP");
        assert!(normalized_query(&"x ".repeat(400)).unwrap().chars().count() <= MAX_QUERY_CHARS);
    }

    #[test]
    fn rejects_empty_query() {
        assert!(normalized_query(" \n ").is_err());
    }
}
