use sqlx::{PgPool, Row};

use teloxide_antispam::text::{jaccard, token_set};

/// DB-доступ для first-message скоринга. Чистый скоринг живёт в
/// `teloxide_antispam::scoring`, здесь только SQL-корпус: template-тексты
/// помеченных сообщений и similarity по персистнутым эмбеддингам.
pub(crate) async fn template_match_count(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
    text: &str,
) -> anyhow::Result<i32> {
    let rows = sqlx::query(
        r#"
        select distinct m.text
        from telegram_messages m
        where m.chat_id = $1
          and m.spam_marked_at is not null
          and m.user_id <> $2
          and m.text is not null
        "#,
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    let current = token_set(text);
    Ok(rows
        .into_iter()
        .filter_map(|row| row.get::<Option<String>, _>("text"))
        .filter(|candidate| jaccard(&current, &token_set(candidate)) >= 0.5)
        .count()
        .min(10) as i32)
}

const SPAM_SIMILARITY_SQL: &str = r#"
    select max(1.0 - (a.first_message_embedding <=> $1::vector))
    from telegram_new_user_profile_audits a
    join telegram_chat_users u
      on u.chat_id = a.chat_id and u.telegram_user_id = a.telegram_user_id
    where u.is_spammer
      and a.first_message_embedding is not null
      and a.telegram_user_id <> $2
    "#;

pub(crate) async fn spam_similarity(
    pool: &PgPool,
    candidate_user_id: i64,
    embedding: &str,
) -> anyhow::Result<Option<f64>> {
    let value = sqlx::query_scalar::<_, Option<f64>>(SPAM_SIMILARITY_SQL)
        .bind(embedding)
        .bind(candidate_user_id)
        .fetch_one(pool)
        .await?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spam_similarity_query_excludes_the_candidate_users_own_messages() {
        assert!(SPAM_SIMILARITY_SQL.contains("a.telegram_user_id <> $2"));
    }
}
