use serde::{Deserialize, Serialize};
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

const MAX_QUERY_CHARS: usize = 240;
const MAX_RESULT_LIMIT: i64 = 20;
const MAX_CONTEXT_MESSAGES: i64 = 5;

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
            greatest(
                ts_rank_cd(
                    to_tsvector('russian', coalesce(m.text, '')),
                    websearch_to_tsquery('russian', $2)
                ),
                ts_rank_cd(
                    to_tsvector('simple', coalesce(m.text, '')),
                    websearch_to_tsquery('simple', $2)
                )
            ) as relevance
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1
          and m.text is not null
          and m.deleted_by_bot_at is null
          and m.spam_marked_at is null
          and (
              to_tsvector('russian', coalesce(m.text, '')) @@ websearch_to_tsquery('russian', $2)
              or to_tsvector('simple', coalesce(m.text, '')) @@ websearch_to_tsquery('simple', $2)
          )
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
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| ChatMessage {
            source_id: source_id(row.message_id),
            message_url: message_url(request.chat_id, row.message_id),
            relevance: (row.relevance * 1000.0).round() as i32,
            message_id: row.message_id,
            user_id: row.user_id,
            author: row.author,
            author_url: author_url(row.author_username.as_deref()),
            text: first_chars(&row.text, 700),
            reply_to_message_id: row.reply_to_message_id,
            created_at: row.created_at.to_rfc3339(),
        })
        .collect())
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

    Ok(rows
        .into_iter()
        .map(|row| ChatMessage {
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
        })
        .collect())
}

pub async fn reply_thread(
    pool: &PgPool,
    chat_id: i64,
    message_id: i32,
) -> anyhow::Result<Vec<ChatMessage>> {
    let rows = sqlx::query_as::<_, MessageRow>(r#"
        with recursive chain as (
            select m.message_id, m.user_id, m.text, m.reply_to_message_id, m.created_at, 0 as depth
            from telegram_messages m where m.chat_id = $1 and m.message_id = $2
            union all
            select parent.message_id, parent.user_id, parent.text, parent.reply_to_message_id, parent.created_at, chain.depth + 1
            from telegram_messages parent join chain on chain.reply_to_message_id = parent.message_id
            where parent.chat_id = $1 and chain.depth < 5
        )
        select chain.message_id, chain.user_id, coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''), nullif(p.username, ''), 'Неизвестный пользователь') as author,
               nullif(p.username, '') as author_username,
               coalesce(chain.text, '[медиа без текста]') as text, chain.reply_to_message_id, chain.created_at, 0::real as relevance
        from chain left join telegram_user_profiles p on p.telegram_user_id = chain.user_id
        order by chain.created_at asc, chain.message_id asc
    "#).bind(chat_id).bind(message_id).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|row| ChatMessage {
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
        })
        .collect())
}

pub async fn user_interactions(
    pool: &PgPool,
    chat_id: i64,
    first_user_id: i64,
    second_user_id: i64,
    limit: i64,
) -> anyhow::Result<Vec<ChatMessage>> {
    let rows = sqlx::query_as::<_, MessageRow>(
        r#"
        select m.message_id, m.user_id,
               coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''),
                        nullif(p.username, ''), 'Неизвестный пользователь') as author,
               nullif(p.username, '') as author_username,
               coalesce(m.text, '[медиа без текста]') as text,
               m.reply_to_message_id, m.created_at, 0::real as relevance
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
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
        .map(|row| ChatMessage {
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
        })
        .collect())
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
