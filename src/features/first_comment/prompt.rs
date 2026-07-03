use crate::features::memory::service::MemoryNote;

pub fn build_llm_prompt(
    post_text: &str,
    chat_member_count: Option<u32>,
    memory_notes: &[MemoryNote],
    recent_comments: &[String],
) -> String {
    let system_prompt = include_str!("../../../prompts/first_comment.md");
    let tech_rag = include_str!("../../../prompts/tech_rag.md");
    let chat_context = match chat_member_count {
        Some(count) => format!(
            "В чате сейчас {count} участников. Это реальное число из Telegram API, но используй его редко."
        ),
        None => "Число участников чата неизвестно, не называй конкретное количество.".to_string(),
    };
    let memory_context = render_memory_context(memory_notes);
    let recent_context = render_recent_comment_context(recent_comments);

    format!(
        "{system_prompt}\n\nRAG для факт-чека, не пересказывать:\n{tech_rag}\n\nПамять прошлых новостей, использовать только если релевантно:\n{memory_context}\n\nПоследние комментарии бота, не повторять стиль и CTA:\n{recent_context}\n\nКонтекст чата:\n{chat_context}\n\nПост:\n{post_text}"
    )
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

    #[test]
    fn recent_comments_are_stripped_from_html() {
        let context = render_recent_comment_context(&["<b>тест</b> <a href=\"x\">чат</a>".into()]);

        assert_eq!(context, "- тест чат");
    }

    #[test]
    fn empty_memory_context_is_explicit() {
        assert_eq!(render_memory_context(&[]), "Нет релевантных заметок.");
    }
}
