pub fn first_text_chars(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }

    trimmed.chars().take(limit).collect::<String>()
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
        .filter(|word| !word.starts_with("http://") && !word.starts_with("https://"))
        .collect::<Vec<_>>()
        .join(" ")
}
