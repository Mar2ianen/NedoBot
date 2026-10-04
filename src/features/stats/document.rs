//! Typed report sections shared by the HTML and Rich renderers.
//!
//! Instead of one `format!` with twenty positional arguments, renderers build
//! `Section`s row by row and let each dialect emit them. Labels come from
//! [`StatsStrings`]; values are pre-formatted fragments owned by the caller.

/// One `label: value` row. Both parts are already final strings; escaping is
/// the caller's job (values) or unnecessary (static labels).
pub struct Kv<'a> {
    pub label: &'a str,
    pub value: String,
}

/// A titled block of rows. `title == None` emits bare rows (used for the
/// report header where the title line is custom).
pub struct Section<'a> {
    pub title: Option<&'a str>,
    pub rows: Vec<Kv<'a>>,
}

impl<'a> Section<'a> {
    /// Telegram HTML dialect: `<b>Title</b>` followed by `label: value` lines.
    pub fn emit_html(&self) -> String {
        let mut output = String::new();
        if let Some(title) = self.title {
            output.push_str(&format!("<b>{title}</b>\n"));
        }
        output.push_str(
            &self
                .rows
                .iter()
                .map(|row| format!("{}: {}", row.label, row.value))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        output
    }

    /// Rich-dialect rows for `table_no_header`: each row becomes
    /// `[label, value]`.
    pub fn emit_rich_rows(&self) -> Vec<Vec<String>> {
        self.rows
            .iter()
            .map(|row| vec![row.label.to_string(), row.value.clone()])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_emit_both_dialects() {
        let section = Section {
            title: Some("Сводка"),
            rows: vec![Kv {
                label: "Сообщения",
                value: "<b>5</b>".to_string(),
            }],
        };
        assert_eq!(section.emit_html(), "<b>Сводка</b>\nСообщения: <b>5</b>");
        assert_eq!(
            section.emit_rich_rows(),
            vec![vec!["Сообщения".to_string(), "<b>5</b>".to_string()]]
        );
    }
}
