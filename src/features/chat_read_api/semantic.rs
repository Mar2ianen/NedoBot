//! Additional semantic read operations shared by RMCP transports.

use serde::Serialize;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Serialize, FromRow)]
pub struct ResolvedUser {
    pub telegram_user_id: i64,
    pub username: Option<String>,
    pub display_name: String,
    pub message_count: i64,
    #[sqlx(skip)]
    pub match_kind: String,
    #[sqlx(skip)]
    pub recommended: bool,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Note {
    pub id: i64,
    pub note: String,
    pub created_by_user_id: i64,
    pub created_at: String,
}

pub async fn resolve_users(
    pool: &PgPool,
    chat_id: i64,
    telegram_user_id: Option<i64>,
    query: Option<&str>,
) -> anyhow::Result<Vec<ResolvedUser>> {
    let query = query.map(normalize_query).transpose()?;
    if telegram_user_id.is_none() && query.is_none() {
        anyhow::bail!("query or telegram_user_id is required");
    }
    let variants = query.as_deref().map(query_variants);
    let rows = sqlx::query_as::<_, ResolveRow>(r#"
        select p.telegram_user_id, nullif(p.username, '') as username,
               coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''), nullif(p.username, ''), 'Неизвестный пользователь') as display_name,
               case when p.telegram_user_id = $2 then 0 when lower(coalesce(p.username, '')) = any(coalesce($3::text[], array[]::text[])) then 1
                    when regexp_replace(lower(concat_ws(' ', p.first_name, p.last_name)), '[^[:alnum:]_]+', '', 'g') = any(coalesce($3::text[], array[]::text[])) then 2 else 3 end as match_rank,
               coalesce(cu.message_count, 0) as message_count
        from mcp_public.telegram_user_profiles p left join mcp_public.telegram_chat_users cu on cu.chat_id = $1 and cu.telegram_user_id = p.telegram_user_id
        where exists (select 1 from mcp_public.telegram_messages m where m.chat_id = $1 and m.user_id = p.telegram_user_id)
          and ($2::bigint is null or p.telegram_user_id = $2)
          and ($3::text[] is null or exists (select 1 from unnest($3::text[]) candidate where position(lower(candidate) in lower(concat_ws(' ', p.username, p.first_name, p.last_name))) > 0 or position(candidate in regexp_replace(lower(concat_ws(' ', p.username, p.first_name, p.last_name)), '[^[:alnum:]_]+', '', 'g')) > 0))
        order by case when p.telegram_user_id = $2 then 0 else 1 end, case when lower(coalesce(p.username, '')) = any(coalesce($3::text[], array[]::text[])) then 0 else 1 end, case when regexp_replace(lower(concat_ws(' ', p.first_name, p.last_name)), '[^[:alnum:]_]+', '', 'g') = any(coalesce($3::text[], array[]::text[])) then 0 else 1 end, coalesce(cu.message_count, 0) desc, p.last_seen_at desc limit 10
    "#).bind(chat_id).bind(telegram_user_id).bind(&variants).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| ResolvedUser {
            telegram_user_id: row.telegram_user_id,
            username: row.username,
            display_name: row.display_name,
            message_count: row.message_count,
            match_kind: match row.match_rank {
                0 => "telegram_id",
                1 => "username",
                2 => "exact_name",
                _ => "partial_name",
            }
            .into(),
            recommended: index == 0,
        })
        .collect())
}

pub async fn chat_notes(pool: &PgPool, chat_id: i64) -> anyhow::Result<Vec<Note>> {
    notes(pool, chat_id, None).await
}
pub async fn user_notes(pool: &PgPool, chat_id: i64, user_id: i64) -> anyhow::Result<Vec<Note>> {
    notes(pool, chat_id, Some(user_id)).await
}

async fn notes(pool: &PgPool, chat_id: i64, user_id: Option<i64>) -> anyhow::Result<Vec<Note>> {
    let sql = if user_id.is_some() {
        "select id, note, created_by_user_id, created_at::text as created_at from mcp_public.telegram_user_notes where chat_id = $1 and telegram_user_id = $2 and status = 'active' order by created_at desc limit 20"
    } else {
        "select id, note, created_by_user_id, created_at::text as created_at from mcp_public.telegram_chat_notes where chat_id = $1 and status = 'active' order by created_at desc limit 20"
    };
    let query = sqlx::query_as::<_, Note>(sql).bind(chat_id);
    if let Some(user_id) = user_id {
        Ok(query.bind(user_id).fetch_all(pool).await?)
    } else {
        Ok(query.fetch_all(pool).await?)
    }
}

#[derive(FromRow)]
struct ResolveRow {
    telegram_user_id: i64,
    username: Option<String>,
    display_name: String,
    match_rank: i32,
    message_count: i64,
}

fn normalize_query(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("query must not be empty");
    }
    Ok(value.chars().take(80).collect())
}
fn query_variants(value: &str) -> Vec<String> {
    let normalized = value.trim().trim_start_matches('@').to_lowercase();
    let compact = normalized
        .chars()
        .filter(|ch| ch.is_alphanumeric() || *ch == '_')
        .collect::<String>();
    let mut values = vec![normalized, compact];
    values.retain(|value| !value.is_empty());
    values.sort();
    values.dedup();
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_resolve_query_is_rejected() {
        assert!(normalize_query(" ").is_err());
    }
    #[test]
    fn resolve_variants_drop_at_sign() {
        assert_eq!(query_variants("@Alice"), vec!["alice"]);
    }
}
