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

pub async fn add_user_note_from_explicit_request(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
    author_id: i64,
    note: &str,
    question: &str,
    command_message_id: i32,
) -> anyhow::Result<()> {
    if !explicit_note_is_safe(question, note) {
        anyhow::bail!("explicit note is not supported by a safe memory request");
    }
    add_user_note_from_search(
        pool,
        chat_id,
        user_id,
        author_id,
        note,
        &[command_message_id],
    )
    .await
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
        "api key",
        "api-key",
        "api_key",
        "api token",
        "auth token",
        "access token",
        "refresh token",
        "token",
        "bearer",
        "secret",
        "секрет",
        "токен",
        "ключ api",
        "api ключ",
    ];
    let normalized_note = note.to_lowercase();
    if sensitive_markers
        .iter()
        .any(|marker| normalized_note.contains(marker))
        || note
            .split(|character: char| !character.is_alphanumeric())
            .any(|token| {
                let digits = token.chars().filter(char::is_ascii_digit).count();
                token.chars().count() >= 7 && digits >= 3
            })
        || note.split_whitespace().any(secret_like_identifier)
    {
        return false;
    }
    let normalized_question = question.to_lowercase();
    let normalized_note_terms = note.to_lowercase();
    let note_terms = normalized_note_terms
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut clause = Vec::new();
    for token in normalized_question.split(|character: char| !character.is_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        if is_fact_clause_separator(token) {
            if note_matches_clause(&clause, &note_terms, note) {
                return true;
            }
            clause.clear();
        } else {
            clause.push(token.to_owned());
        }
    }
    note_matches_clause(&clause, &note_terms, note)
}

fn is_fact_clause_separator(token: &str) -> bool {
    matches!(
        token,
        "но" | "а" | "и" | "или" | "либо" | "but" | "and" | "or" | "however"
    )
}

fn note_matches_clause(clause: &[String], note_terms: &[String], note: &str) -> bool {
    !note_terms.is_empty()
        && clause
            .windows(note_terms.len())
            .any(|window| window == note_terms)
        && negation_signature(&clause.join(" ")) == negation_signature(note)
}

fn negation_signature(text: &str) -> Vec<String> {
    const NEGATIONS: &[&str] = &[
        "не",
        "нет",
        "никогда",
        "ни",
        "без",
        "not",
        "no",
        "never",
        "don't",
        "dont",
        "doesn't",
        "doesnt",
        "didn't",
        "didnt",
        "can't",
        "cant",
        "cannot",
        "won't",
        "wont",
    ];
    text.to_lowercase()
        .split(|character: char| !character.is_alphanumeric() && character != '\'')
        .filter(|token| NEGATIONS.contains(token))
        .map(str::to_owned)
        .collect()
}

fn secret_like_identifier(value: &str) -> bool {
    let value = value.trim_matches(|character: char| !character.is_alphanumeric());
    let has_separator = value.contains('-') || value.contains('_');
    has_separator
        && value.chars().count() >= 6
        && value
            .chars()
            .any(|character| character.is_ascii_alphabetic())
        && value.chars().any(|character| character.is_ascii_digit())
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
        assert!(!explicit_note_is_safe(
            "Запомни мой тестовый идентификатор abc-123",
            "тестовый идентификатор abc-123"
        ));
        assert!(!explicit_note_is_safe(
            "Запомни мой API key abc-123",
            "мой API key abc-123"
        ));
        assert!(!explicit_note_is_safe(
            "Запомни: я люблю Rust",
            "я не люблю Rust"
        ));
        assert!(!explicit_note_is_safe(
            "Запомни: я не люблю Rust",
            "я люблю Rust"
        ));
        assert!(explicit_note_is_safe(
            "Запомни: я не люблю Rust",
            "не люблю Rust"
        ));
        let multi_fact_request = "Запомни: я не люблю Rust, но люблю Go";
        assert!(explicit_note_is_safe(multi_fact_request, "не люблю Rust"));
        assert!(explicit_note_is_safe(multi_fact_request, "люблю Go"));
        assert!(!explicit_note_is_safe(multi_fact_request, "не люблю Go"));
    }
}
