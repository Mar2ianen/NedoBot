use sqlx::PgPool;

const MAX_NOTE_CHARS: usize = 1200;

pub async fn add_chat_note(
    pool: &PgPool,
    chat_id: i64,
    author_id: i64,
    note: &str,
) -> anyhow::Result<()> {
    let note = normalize_note(note)?;
    sqlx::query(
        "insert into telegram_chat_notes (chat_id, note, created_by_user_id) values ($1, $2, $3)",
    )
    .bind(chat_id)
    .bind(note)
    .bind(author_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn add_user_note(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
    author_id: i64,
    note: &str,
) -> anyhow::Result<()> {
    let note = normalize_note(note)?;
    sqlx::query("insert into telegram_user_notes (chat_id, telegram_user_id, note, created_by_user_id) values ($1, $2, $3, $4)")
        .bind(chat_id).bind(user_id).bind(note).bind(author_id).execute(pool).await?;
    Ok(())
}

fn normalize_note(note: &str) -> anyhow::Result<String> {
    let note = note.split_whitespace().collect::<Vec<_>>().join(" ");
    if note.is_empty() {
        anyhow::bail!("note must not be empty");
    }
    if note.chars().count() > MAX_NOTE_CHARS {
        anyhow::bail!("note exceeds limit");
    }
    Ok(note)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_note() {
        assert_eq!(normalize_note("  важный\n факт ").unwrap(), "важный факт");
    }
}
