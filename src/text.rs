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

/// Replaces Latin letters visually indistinguishable from Cyrillic ones, but
/// only in mixed Cyrillic/Latin text. Pure Latin identifiers stay unchanged.
pub fn normalize_cyrillic_homoglyphs(text: &str) -> String {
    if !text.chars().any(|ch| matches!(ch, '\u{0400}'..='\u{04ff}')) {
        return text.to_string();
    }
    text.chars()
        .map(|ch| match ch {
            'A' => 'А',
            'B' => 'В',
            'C' => 'С',
            'E' => 'Е',
            'H' => 'Н',
            'K' => 'К',
            'M' => 'М',
            'O' => 'О',
            'P' => 'Р',
            'T' => 'Т',
            'X' => 'Х',
            'Y' => 'У',
            'a' => 'а',
            'c' => 'с',
            'e' => 'е',
            'o' => 'о',
            'p' => 'р',
            'x' => 'х',
            'y' => 'у',
            _ => ch,
        })
        .collect()
}

pub fn has_mixed_script_homoglyphs(text: &str) -> bool {
    normalize_cyrillic_homoglyphs(text) != text
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

    #[test]
    fn normalizes_latin_homoglyph_in_cyrillic_name() {
        assert_eq!(normalize_cyrillic_homoglyphs("Tанюша"), "Танюша");
        assert!(has_mixed_script_homoglyphs("Tанюша"));
        assert_eq!(normalize_cyrillic_homoglyphs("Alice"), "Alice");
    }
}
