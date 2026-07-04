pub fn validate_comment_output(text: &str) -> anyhow::Result<()> {
    let normalized = text.trim();
    if normalized.is_empty() {
        anyhow::bail!("empty first comment output");
    }

    let chat_link_count = normalized.matches("{CHAT_LINK").count();
    if chat_link_count == 0 {
        anyhow::bail!("first comment is missing CHAT_LINK placeholder");
    }
    if chat_link_count > 1 {
        anyhow::bail!("first comment contains multiple CHAT_LINK placeholders");
    }

    let visible = normalized
        .replace("{CHAT_LINK}", "")
        .replace("{CHAT_LINK:", "")
        .replace('}', " ");
    let visible_len = visible.chars().filter(|ch| !ch.is_whitespace()).count();
    if visible_len > 180 {
        anyhow::bail!("first comment is too long: {visible_len} visible chars");
    }

    let words = visible
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .count();
    if words < 5 {
        anyhow::bail!("first comment is too short: {words} words");
    }
    if words > 30 {
        anyhow::bail!("first comment is too wordy: {words} words");
    }

    let sentence_marks = visible
        .chars()
        .filter(|ch| matches!(*ch, '.' | '!' | '?'))
        .count();
    if sentence_marks > 2 {
        anyhow::bail!("first comment has too many sentences");
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

    let generic_phrases = [
        "давайте обсудим",
        "обсудим это",
        "обсудим новость",
        "обсудим эту новость",
        "обсудим эту тему",
        "поделитесь мнением",
        "интересная новость",
        "важная тема",
        "что думаете",
        "пишите в комментариях",
        "заходите",
        "залетайте",
    ];
    if let Some(phrase) = generic_phrases
        .iter()
        .find(|phrase| lower.contains(**phrase))
    {
        anyhow::bail!("first comment contains generic CTA phrase: {phrase}");
    }

    let has_substantive_cyrillic_word = visible.split_whitespace().any(|word| {
        word.chars()
            .filter(|ch| matches!(*ch, '\u{0400}'..='\u{04FF}'))
            .count()
            >= 5
    });
    if !has_substantive_cyrillic_word {
        anyhow::bail!("first comment has no substantive Russian topic word");
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
        assert!(validate_comment_output("re-released {CHAT_LINK}").is_err());
    }

    #[test]
    fn rejects_raw_links() {
        assert!(
            validate_comment_output("Переиздания обсудим в чате https://t.me/example {CHAT_LINK}")
                .is_err()
        );
    }

    #[test]
    fn rejects_missing_placeholder() {
        assert!(
            validate_comment_output("Физические релизы превращаются в архивный формат.").is_err()
        );
    }

    #[test]
    fn rejects_duplicate_placeholder() {
        assert!(validate_comment_output(
            "Физические релизы превращаются в архивный формат. {CHAT_LINK} Детали в {CHAT_LINK:чате}.",
        )
        .is_err());
    }

    #[test]
    fn rejects_long_generic_comment() {
        assert!(validate_comment_output(
            "Интересная новость про рынок технологий, давайте обсудим это подробнее и поделитесь мнением, потому что тема важная для всех участников. Подробности в {CHAT_LINK:чате}.",
        )
        .is_err());
    }

    #[test]
    fn accepts_normal_comment() {
        validate_comment_output("Физические релизы превращаются в архивный формат. Коллекции и цены в {CHAT_LINK:чате}.").unwrap();
    }
}
