const MAX_VISIBLE_CHARS: usize = 180;
const MAX_WORDS: usize = 30;
const MAX_SENTENCE_MARKS: usize = 2;
const MIN_WORDS: usize = 5;
const MIN_CYRILLIC_CHARS: usize = 8;

const VICTIM_PHRASES: &[&str] = &[
    "из нас последнее",
    "из нас выжимают",
    "кошельки плачут",
    "нас опять заставляют",
    "нас снова заставляют",
    "нам ждать",
    "придется ждать",
    "придётся ждать",
];

const GENERIC_PHRASES: &[&str] = &[
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

const ALLOWED_CHAT_LINK_LABELS: &[&str] = &[
    "чат",
    "чате",
    "чатик",
    "чатике",
    "обсуждение",
    "обсуждении",
    "комменты",
    "комментах",
];

pub fn validate_comment_output(text: &str) -> anyhow::Result<()> {
    let normalized = text.trim();
    if normalized.is_empty() {
        anyhow::bail!("empty first comment output");
    }
    if ends_with_forbidden_final_punctuation(normalized) {
        anyhow::bail!("first comment must not end with a dot or ellipsis");
    }

    let placeholders = scan_chat_link_placeholders(normalized)?;
    if placeholders.valid_count == 0 {
        anyhow::bail!("first comment is missing valid CHAT_LINK placeholder");
    }
    if placeholders.valid_count > 1 {
        anyhow::bail!("first comment contains multiple CHAT_LINK placeholders");
    }

    let visible = placeholders.visible_text;
    let visible_len = visible.chars().filter(|ch| !ch.is_whitespace()).count();
    if visible_len > MAX_VISIBLE_CHARS {
        anyhow::bail!("first comment is too long: {visible_len} visible chars");
    }

    let words = visible
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .count();
    if words < MIN_WORDS {
        anyhow::bail!("first comment is too short: {words} words");
    }
    if words > MAX_WORDS {
        anyhow::bail!("first comment is too wordy: {words} words");
    }

    let sentence_marks = visible
        .chars()
        .filter(|ch| matches!(*ch, '.' | '!' | '?'))
        .count();
    if sentence_marks > MAX_SENTENCE_MARKS {
        anyhow::bail!("first comment has too many sentences");
    }

    let cyrillic = visible
        .chars()
        .filter(|ch| matches!(*ch, '\u{0400}'..='\u{04FF}'))
        .count();
    if cyrillic < MIN_CYRILLIC_CHARS {
        anyhow::bail!("first comment has too little Cyrillic text");
    }

    let lower = visible.to_lowercase();
    if lower.contains("http://") || lower.contains("https://") || lower.contains("t.me/") {
        anyhow::bail!("first comment contains a raw link");
    }

    if let Some(phrase) = GENERIC_PHRASES
        .iter()
        .find(|phrase| lower.contains(**phrase))
    {
        anyhow::bail!("first comment contains generic CTA phrase: {phrase}");
    }
    if let Some(phrase) = VICTIM_PHRASES
        .iter()
        .find(|phrase| lower.contains(**phrase))
    {
        anyhow::bail!("first comment sounds like a victim complaint: {phrase}");
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

struct PlaceholderScan {
    valid_count: usize,
    visible_text: String,
}

fn ends_with_forbidden_final_punctuation(text: &str) -> bool {
    let trimmed = text.trim_end();
    trimmed.ends_with('.') || trimmed.ends_with('…')
}

fn scan_chat_link_placeholders(text: &str) -> anyhow::Result<PlaceholderScan> {
    let mut valid_count = 0;
    let mut visible = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find("{CHAT_LINK") {
        let (before, after_start) = rest.split_at(start);
        visible.push_str(before);

        let Some(end) = after_start.find('}') else {
            anyhow::bail!("first comment contains unterminated CHAT_LINK placeholder");
        };

        let token = &after_start[..=end];
        validate_chat_link_token(token)?;
        valid_count += 1;
        visible.push(' ');
        rest = &after_start[end + 1..];
    }

    visible.push_str(rest);
    Ok(PlaceholderScan {
        valid_count,
        visible_text: visible,
    })
}

fn validate_chat_link_token(token: &str) -> anyhow::Result<()> {
    if token == "{CHAT_LINK}" {
        return Ok(());
    }

    let Some(label) = token
        .strip_prefix("{CHAT_LINK:")
        .and_then(|value| value.strip_suffix('}'))
        .map(str::trim)
    else {
        anyhow::bail!("first comment contains malformed CHAT_LINK placeholder: {token}");
    };

    if ALLOWED_CHAT_LINK_LABELS.contains(&label) {
        Ok(())
    } else {
        anyhow::bail!("first comment contains unsupported CHAT_LINK label: {label}");
    }
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
            validate_comment_output(
                "Физические релизы превращаются в архивный формат. Охота за коробками началась"
            )
            .is_err()
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
    fn rejects_malformed_placeholder() {
        assert!(validate_comment_output(
            "Физические релизы превращаются в архивный формат. Коллекции и цены в {CHAT_LINKED}.",
        )
        .is_err());
    }

    #[test]
    fn rejects_unsupported_placeholder_label() {
        assert!(validate_comment_output(
            "Физические релизы превращаются в архивный формат. Коллекции и цены в {CHAT_LINK:в чате}.",
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
    fn rejects_final_dot() {
        assert!(
            validate_comment_output(
                "ИИ опять крайний, а 32 гб в минималках это типа само выросло. Прайсы в {CHAT_LINK}.",
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_final_ellipsis() {
        assert!(
            validate_comment_output(
                "ИИ опять крайний, а системки игр подозрительно молчат. Прайсы в {CHAT_LINK}…",
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_victim_tone() {
        assert!(
            validate_comment_output("ИИ опять выжимает из нас последнее. Прайсы в {CHAT_LINK}")
                .is_err()
        );
    }

    #[test]
    fn accepts_normal_comment() {
        validate_comment_output(
            "Физические релизы превращаются в архивный формат. Коллекции и цены в {CHAT_LINK:чате}",
        )
        .unwrap();
    }
}
