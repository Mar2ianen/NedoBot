use crate::features::memory::service::MemoryNote;
use crate::features::search::types::{SearchContext, SearchSource};

pub fn build_llm_prompt(
    post_text: &str,
    chat_member_count: Option<u32>,
    memory_notes: &[MemoryNote],
    recent_comments: &[String],
    search_context: Option<&SearchContext>,
) -> String {
    let system_prompt = include_str!("../../../prompts/first_comment.md");
    let tech_rag = include_str!("../../../prompts/tech_rag.md");
    let chat_context = match chat_member_count {
        Some(count) => format!(
            "В чате сейчас {count} участников. Это реальное число из Telegram API, но используй его редко."
        ),
        None => "Число участников чата неизвестно, не называй конкретное количество.".to_string(),
    };
    let search_context = render_search_context(search_context);
    let memory_context = render_memory_context(memory_notes);
    let recent_context = render_recent_comment_context(recent_comments);

    format!(
        "{system_prompt}\n\nRAG для факт-чека, не пересказывать:\n{tech_rag}{search_context}\n\nПамять прошлых новостей, использовать только если релевантно:\n{memory_context}\n\nПоследние комментарии бота, не повторять стиль и CTA:\n{recent_context}\n\nКонтекст чата:\n{chat_context}\n\nПост:\n{post_text}"
    )
}

fn render_search_context(search_context: Option<&SearchContext>) -> String {
    let Some(search_context) = search_context else {
        return String::new();
    };

    if search_context.is_skipped() || search_context.results.is_empty() {
        return "\n\nСвежий поиск: нет дополнительного контекста.".to_string();
    }

    let results = search_context
        .results
        .iter()
        .map(|result| {
            format!(
                "- [{}] {} — {}",
                search_source_label(result.source),
                result.title,
                result.snippet
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "\n\nСвежий поиск, использовать осторожно:\n- Это вспомогательный контекст, он ниже поста по приоритету.\n- Не цитируй URL и не добавляй ссылки.\n- Если поиск противоречит посту, не утверждай спорное как факт.\n- Если результаты нерелевантны, игнорируй их.\n\nРезультаты:\n{results}"
    )
}

fn search_source_label(source: SearchSource) -> &'static str {
    match source {
        SearchSource::Web => "web",
        SearchSource::Github => "github",
        SearchSource::Reddit => "reddit",
    }
}

fn render_memory_context(memory_notes: &[MemoryNote]) -> String {
    if memory_notes.is_empty() {
        return "Нет релевантных заметок.".to_string();
    }

    memory_notes
        .iter()
        .take(5)
        .map(|note| {
            format!(
                "- {}: {}{}",
                note.title,
                note.summary,
                if note.cautions.trim().is_empty() {
                    String::new()
                } else {
                    format!(" Осторожно: {}", note.cautions)
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_recent_comment_context(recent_comments: &[String]) -> String {
    if recent_comments.is_empty() {
        return "Нет последних комментариев.".to_string();
    }

    recent_comments
        .iter()
        .take(12)
        .map(|comment| format!("- {}", strip_html_tags(comment)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_html_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_tag = false;

    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::search::types::{SearchQuery, SearchResult};

    #[test]
    fn recent_comments_are_stripped_from_html() {
        let context = render_recent_comment_context(&["<b>тест</b> <a href=\"x\">чат</a>".into()]);

        assert_eq!(context, "- тест чат");
    }

    #[test]
    fn empty_memory_context_is_explicit() {
        assert_eq!(render_memory_context(&[]), "Нет релевантных заметок.");
    }

    #[test]
    fn absent_search_context_keeps_search_block_out() {
        let prompt = build_llm_prompt("Пост", None, &[], &[], None);

        assert!(!prompt.contains("Свежий поиск"));
    }

    #[test]
    fn search_context_is_rendered_without_urls() {
        let search_context = SearchContext {
            queries: vec![SearchQuery {
                source: SearchSource::Web,
                text: "Rust release".to_string(),
            }],
            results: vec![SearchResult {
                source: SearchSource::Web,
                title: "Rust 1.90 released".to_string(),
                url: "https://example.com/rust".to_string(),
                snippet: "Release notes mention compiler improvements.".to_string(),
            }],
            skipped_reason: None,
            latency_ms: 42,
        };

        let prompt = build_llm_prompt("Пост", None, &[], &[], Some(&search_context));

        assert!(prompt.contains("Свежий поиск, использовать осторожно:"));
        assert!(
            prompt.contains(
                "- [web] Rust 1.90 released — Release notes mention compiler improvements."
            )
        );
        assert!(!prompt.contains("https://example.com/rust"));
    }

    #[test]
    fn skipped_search_context_is_explicit() {
        let search_context = SearchContext::skipped("no_search_needed", 10);

        let prompt = build_llm_prompt("Пост", None, &[], &[], Some(&search_context));

        assert!(prompt.contains("Свежий поиск: нет дополнительного контекста."));
    }
}
