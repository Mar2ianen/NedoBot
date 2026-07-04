pub fn first_text_chars(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }

    if limit == 0 {
        return "…".to_string();
    }

    format!("{}…", trimmed.chars().take(limit).collect::<String>())
}

pub fn normalize_ai_markers(text: &str) -> String {
    text.replace(['—', '–'], "-")
        .replace(['«', '»'], "\"")
        .replace("Вот вариант:", "")
        .replace("Вариант:", "")
        .trim()
        .to_string()
}

pub fn strip_links(text: &str) -> String {
    text.split_whitespace()
        .filter(|word| {
            let trimmed = word.trim_matches(|ch: char| {
                ch.is_ascii_punctuation()
                    || matches!(ch, '«' | '»' | '“' | '”' | '„' | '‹' | '›' | '【' | '】')
            });
            !trimmed.starts_with("http://") && !trimmed.starts_with("https://")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_text_chars_marks_truncation() {
        assert_eq!(first_text_chars("abcdef", 3), "abc…");
        assert_eq!(first_text_chars("abc", 3), "abc");
    }

    #[test]
    fn strip_links_handles_wrapping_punctuation() {
        assert_eq!(strip_links("смотри (https://example.com), ок"), "смотри ок");
        assert_eq!(strip_links("смотри https://example.com. ок"), "смотри ок");
    }
}
