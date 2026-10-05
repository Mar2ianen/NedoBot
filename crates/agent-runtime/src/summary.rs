//! Структурный summary (Stage 3 компактификации).
//!
//! Summary хранится структурой, а не строкой: рендер в model-specific
//! текст — отдельная функция, summary можно перепроверить и пересуммировать.

use serde::{Deserialize, Serialize};

/// Структурный итог диапазона истории.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub task: String,
    pub decisions: Vec<String>,
    pub constraints: Vec<String>,
    pub completed_work: Vec<String>,
    pub open_work: Vec<String>,
    pub files: Vec<FileState>,
    pub relevant_tool_state: Vec<String>,
}

/// Состояние файла на момент summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    pub path: String,
    pub digest: Option<String>,
    pub note: String,
}

impl ConversationSummary {
    /// Детерминированный рендер для подстановки в контекст.
    /// Формат стабилен, чтобы planner мог оценить размер заранее.
    pub fn render_text(&self) -> String {
        let mut out = String::from("SUMMARY:\n");
        out.push_str(&format!("Задача: {}\n", self.task));
        render_list(&mut out, "Решения", &self.decisions);
        render_list(&mut out, "Ограничения", &self.constraints);
        render_list(&mut out, "Сделано", &self.completed_work);
        render_list(&mut out, "Открыто", &self.open_work);
        if !self.files.is_empty() {
            out.push_str("Файлы:\n");
            for file in &self.files {
                out.push_str(&format!("- {}: {}\n", file.path, file.note));
            }
        }
        render_list(
            &mut out,
            "Состояние инструментов",
            &self.relevant_tool_state,
        );
        out
    }
}

fn render_list(out: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    out.push_str(title);
    out.push_str(":\n");
    for item in items {
        out.push_str("- ");
        out.push_str(item);
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_is_deterministic_and_skips_empty() {
        let summary = ConversationSummary {
            task: "починить сборку".to_owned(),
            decisions: vec!["остаёмся на genai 0.6".to_owned()],
            open_work: vec!["поднять rmcp".to_owned()],
            ..Default::default()
        };
        let first = summary.render_text();
        let second = summary.render_text();
        assert_eq!(first, second);
        assert!(first.contains("починить сборку"));
        assert!(!first.contains("Ограничения"));
    }
}
