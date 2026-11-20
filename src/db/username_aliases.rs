use std::collections::HashMap;

use sqlx::PgPool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTelegramPeer {
    pub peer_id: i64,
    pub peer_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramUsernameResolution {
    Unique(ResolvedTelegramPeer),
    Ambiguous,
    Unknown,
}

pub(crate) async fn resolve_current_username_aliases(
    pool: &PgPool,
    usernames: &[String],
) -> anyhow::Result<HashMap<String, TelegramUsernameResolution>> {
    if usernames.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(String, i64, String, i64)> = sqlx::query_as(
        r#"select username_normalized, min(telegram_peer_id), min(peer_type),
                  count(distinct (telegram_peer_id, peer_type))::bigint
             from public.telegram_username_aliases
            where username_normalized = any($1)
              and is_active
              and observed_at >= now() - interval '24 hours'
            group by username_normalized"#,
    )
    .bind(usernames)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(username, peer_id, peer_type, count)| {
            let resolution = if count == 1 {
                TelegramUsernameResolution::Unique(ResolvedTelegramPeer { peer_id, peer_type })
            } else {
                TelegramUsernameResolution::Ambiguous
            };
            (username, resolution)
        })
        .collect())
}

pub async fn resolve_telegram_username(
    pool: &PgPool,
    username: &str,
) -> anyhow::Result<TelegramUsernameResolution> {
    let Some(username) = normalize_username(username) else {
        return Ok(TelegramUsernameResolution::Unknown);
    };
    let mut resolutions =
        resolve_current_username_aliases(pool, std::slice::from_ref(&username)).await?;
    Ok(resolutions
        .remove(&username)
        .unwrap_or(TelegramUsernameResolution::Unknown))
}

pub(crate) fn normalize_username(value: &str) -> Option<String> {
    let username = value.trim().trim_start_matches('@').to_ascii_lowercase();
    (5..=32).contains(&username.len()).then_some(())?;
    username
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        .then_some(username)
}
