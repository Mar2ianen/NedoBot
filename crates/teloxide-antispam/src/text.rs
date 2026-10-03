use std::collections::BTreeSet;

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

/// Нормализация evidence личного канала для grounded-сравнения:
/// нижний регистр, только буквы/цифры/пробелы, схлопнутые пробелы.
pub fn normalize_channel_evidence(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character.is_whitespace() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Токены первого сообщения для template-матчинга кампаний: слова от 4
/// символов со свёрткой DM/funnel-синонимов в канонические маркеры.
pub fn token_set(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.chars().count() >= 4)
        .map(campaign_token)
        .collect()
}

fn campaign_token(word: &str) -> String {
    match word {
        "отправить"
        | "отправлю"
        | "переслать"
        | "перешлю"
        | "скинуть"
        | "скину"
        | "поделиться"
        | "поделюсь"
        | "закинуть"
        | "закину" => "send_offer".to_string(),
        "личку" | "личные" | "сообщения" | "стучитесь" => {
            "direct_messages".to_string()
        }
        "аудиокнигу" | "аудиокнига" | "аудиоверсия" | "текстовая" => {
            "promoted_material".to_string()
        }
        _ => word.to_owned(),
    }
}

/// Жаккард для template-матчинга: пустое объединение даёт 0.0, чтобы
/// бессодержательные сообщения не матчились друг с другом.
pub fn jaccard(left: &BTreeSet<String>, right: &BTreeSet<String>) -> f64 {
    let union = left.union(right).count();
    if union == 0 {
        0.0
    } else {
        left.intersection(right).count() as f64 / union as f64
    }
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

    #[test]
    fn campaign_tokens_fold_dm_funnel_synonyms() {
        let tokens = token_set("Напишите в личку, скину аудиокнигу бесплатно");
        assert!(tokens.contains("send_offer"));
        assert!(tokens.contains("direct_messages"));
        assert!(tokens.contains("promoted_material"));
    }

    #[test]
    fn jaccard_ignores_empty_sets() {
        let empty = BTreeSet::new();
        assert_eq!(jaccard(&empty, &empty), 0.0);
    }
}
