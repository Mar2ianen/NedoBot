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

pub async fn add_user_note_from_search(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
    author_id: i64,
    note: &str,
    source_message_ids: &[i32],
) -> anyhow::Result<()> {
    let note = normalize_note(note)?;
    if source_message_ids.is_empty() {
        anyhow::bail!("automatic user notes require message sources");
    }
    sqlx::query("insert into telegram_user_notes (chat_id, telegram_user_id, note, created_by_user_id, source_message_ids) select $1, $2, $3, $4, $5 where not exists (select 1 from telegram_user_notes where chat_id = $1 and telegram_user_id = $2 and note = $3 and status = 'active')")
        .bind(chat_id).bind(user_id).bind(&note).bind(author_id).bind(serde_json::json!(source_message_ids)).execute(pool).await?;
    Ok(())
}

pub fn explicit_memory_request(question: &str) -> bool {
    let normalized = question
        .trim()
        .trim_matches(|character: char| !character.is_alphanumeric())
        .to_lowercase();
    if normalized.starts_with("не ")
        || normalized.starts_with("don't ")
        || normalized.starts_with("do not ")
        || normalized.starts_with("не надо ")
    {
        return false;
    }
    [
        "запомни",
        "запиши в память",
        "сохрани в памяти",
        "можешь запомнить",
        "пожалуйста запомни",
        "remember",
        "please remember",
        "make a note",
    ]
    .iter()
    .any(|phrase| normalized.starts_with(phrase))
}

/// Conservatively accepts only notes restated in an explicit request, and
/// rejects obvious sensitive-data categories before a model-generated note is stored.
pub fn explicit_note_is_safe(question: &str, note: &str) -> bool {
    if !explicit_memory_request(question) || note.trim().is_empty() {
        return false;
    }
    let sensitive_markers = [
        "парол",
        "password",
        "телефон",
        "phone number",
        "адрес",
        "address",
        "паспорт",
        "passport",
        "диагноз",
        "болезн",
        "здоров",
        "medical",
        "salary",
        "зарплат",
        "доход",
        "income",
        "кредит",
        "credit card",
        "карта",
        "ssn",
        "социальн",
        "религи",
        "religion",
        "политическ",
        "political",
        "сексуал",
        "sexual",
    ];
    let normalized_note = note.to_lowercase();
    if sensitive_markers
        .iter()
        .any(|marker| normalized_note.contains(marker))
        || note
            .split(|character: char| !character.is_alphanumeric())
            .any(|token| token.chars().filter(char::is_ascii_digit).count() >= 7)
    {
        return false;
    }
    let question_terms = question
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect::<std::collections::HashSet<_>>();
    note.to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.chars().count() >= 3)
        .all(|token| question_terms.contains(token))
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

    #[test]
    fn durable_note_write_requires_a_clear_unnegated_request() {
        assert!(explicit_memory_request(
            "Запомни: я предпочитаю короткие ответы"
        ));
        assert!(explicit_memory_request("Please remember that I use Rust"));
        assert!(!explicit_memory_request("Я обычно отвечаю коротко"));
        assert!(!explicit_memory_request("Не запоминай это"));
        assert!(!explicit_memory_request("The quote says: remember this"));
        assert!(explicit_note_is_safe(
            "Запомни: я предпочитаю короткие ответы",
            "предпочитаю короткие ответы"
        ));
        assert!(!explicit_note_is_safe(
            "Запомни: я предпочитаю короткие ответы",
            "работает врачом и любит путешествия"
        ));
        assert!(!explicit_note_is_safe(
            "Запомни мой номер телефона 123456789",
            "номер телефона 123456789"
        ));
    }
}
