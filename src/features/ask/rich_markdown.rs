use std::collections::HashSet;

const MAX_RICH_MARKDOWN_CHARS: usize = 32_000;

/// Преобразует маркеры цитат модели в ссылки по доверенным alias.
/// Ссылка создаётся только для ID, полученного инструментами этого запуска.
pub fn normalize_message_citations(markdown: &str, observed_message_ids: &[i32]) -> String {
    let observed_message_ids = observed_message_ids.iter().copied().collect::<HashSet<_>>();
    let marker = "【message_";
    let mut output = String::with_capacity(markdown.len());
    let mut remaining = markdown;

    while let Some(marker_start) = remaining.find(marker) {
        let content_start = marker_start + marker.len();
        let Some(marker_end_relative) = remaining[content_start..].find('】') else {
            output.push_str(remaining);
            return output;
        };
        let marker_end = content_start + marker_end_relative;
        let marker_end_exclusive = marker_end + '】'.len_utf8();
        let id_text = &remaining[content_start..marker_end];
        let message_id = id_text.parse::<i32>().ok();

        output.push_str(&remaining[..marker_start]);
        if let Some(message_id) = message_id.filter(|id| observed_message_ids.contains(id)) {
            let alias = format!("message_{message_id}");
            output.push_str(&format!("[{alias}]({alias})"));
        } else {
            output.push_str(&remaining[marker_start..marker_end_exclusive]);
        }
        remaining = &remaining[marker_end_exclusive..];
    }
    output.push_str(remaining);
    output
}

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

    #[test]
    fn links_only_citations_observed_in_this_ask_run() {
        assert_eq!(
            normalize_message_citations(
                "Подтверждение 【message_42】, неизвестно 【message_99】.",
                &[42],
            ),
            "Подтверждение [message_42](message_42), неизвестно 【message_99】."
        );
    }

    #[test]
    fn normalized_citations_are_recognized_as_message_alias_links() {
        let markdown = normalize_message_citations("Факт 【message_425668】.", &[425668]);
        let parsed = teloxide::utils::rich_text::LlmMarkdownFormatter::new()
            .parse(&markdown)
            .unwrap();

        assert_eq!(parsed.link_aliases(), vec!["message_425668"]);
    }

    #[test]
    fn normalized_citations_render_to_the_chat_message_url() {
        use teloxide::utils::{
            rich_text::{LlmMarkdownFormatter, RichTextBindings, RichTextRenderContext},
            time::{TimeBindings, TimeContext},
        };
        use url::Url;

        let markdown = normalize_message_citations("Факт 【message_425668】.", &[425668]);
        let mut bindings = RichTextBindings::new();
        bindings
            .insert_link(
                "message_425668",
                Url::parse("https://t.me/c/1932061163/425668").unwrap(),
            )
            .unwrap();
        let time = TimeContext::from_name("Europe/Moscow").unwrap();
        let time_bindings = TimeBindings::default();
        let context = RichTextRenderContext::for_llm(&time, &time_bindings, &bindings);
        let rendered = LlmMarkdownFormatter::new()
            .render_with_context_at(
                &markdown,
                &context,
                "2026-10-08T00:00:00Z".parse::<jiff::Timestamp>().unwrap(),
            )
            .unwrap();

        assert!(
            rendered
                .compiled
                .contains("https://t.me/c/1932061163/425668")
        );
    }

    #[test]
    fn preserves_existing_markdown_links_and_malformed_markers() {
        assert_eq!(
            normalize_message_citations(
                "[источник](message_7), 【message_not-an-id】 и 【message_8",
                &[7, 8],
            ),
            "[источник](message_7), 【message_not-an-id】 и 【message_8"
        );
    }
}
