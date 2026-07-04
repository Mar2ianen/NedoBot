pub fn validate_comment_output(text: &str) -> anyhow::Result<()> {
    let normalized = text.trim();
    if normalized.is_empty() {
        anyhow::bail!("empty first comment output");
    }

    let visible = normalized
        .replace("{CHAT_LINK}", "")
        .replace("{CHAT_LINK:", "")
        .replace('}', " ");
    let words = visible
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .count();
    if words < 5 {
        anyhow::bail!("first comment is too short: {words} words");
    }

    let cyrillic = visible
        .chars()
        .filter(|ch| matches!(*ch, '\u{0400}'..='\u{04FF}'))
        .count();
    if cyrillic < 8 {
        anyhow::bail!("first comment has too little Cyrillic text");
    }

    let lower = visible.to_lowercase();
    if lower.contains("http://") || lower.contains("https://") || lower.contains("t.me/") {
        anyhow::bail!("first comment contains a raw link");
    }

    let latin = visible
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .count();
    if latin > cyrillic && cyrillic < 20 {
        anyhow::bail!("first comment looks mostly English");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_english_garbage() {
        assert!(validate_comment_output("re-released").is_err());
    }

    #[test]
    fn rejects_raw_links() {
        assert!(
            validate_comment_output("Переиздания обсудим в чате https://t.me/example").is_err()
        );
    }

    #[test]
    fn accepts_normal_comment() {
        validate_comment_output("Физические релизы окончательно превращаются в архивный формат. Переиздания обсудим в {CHAT_LINK:чате}.").unwrap();
    }
}
