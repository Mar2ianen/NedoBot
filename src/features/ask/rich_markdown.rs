const MAX_RICH_MARKDOWN_CHARS: usize = 32_000;

pub fn validate(markdown: &str) -> anyhow::Result<String> {
    let markdown = markdown.trim();
    if markdown.is_empty() {
        anyhow::bail!("ask answer is empty");
    }
    if markdown
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        anyhow::bail!("ask answer contains control characters");
    }
    if markdown.chars().count() > MAX_RICH_MARKDOWN_CHARS {
        anyhow::bail!("ask answer exceeds rich message limit");
    }
    Ok(markdown.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_supported_markdown_text() {
        assert_eq!(
            validate("## Заголовок\n\n**жирный**").unwrap(),
            "## Заголовок\n\n**жирный**"
        );
    }

    #[test]
    fn rejects_hidden_control_characters() {
        assert!(validate("текст\u{0000}").is_err());
    }
}
