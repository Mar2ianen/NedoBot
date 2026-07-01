#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Html(String);

pub const TELEGRAM_TEXT_LIMIT: usize = 4096;
pub const SAFE_TEXT_LIMIT: usize = 3900;

impl Html {
    pub fn empty() -> Self {
        Self(String::new())
    }

    pub fn text(value: impl AsRef<str>) -> Self {
        Self(escape(value.as_ref()))
    }

    pub(crate) fn raw_trusted(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn bold(value: impl AsRef<str>) -> Self {
        Self(format!("<b>{}</b>", escape(value.as_ref())))
    }

    pub fn code(value: impl AsRef<str>) -> Self {
        Self(format!("<code>{}</code>", escape(value.as_ref())))
    }

    pub fn link(label: impl AsRef<str>, url: impl AsRef<str>) -> Self {
        Self(format!(
            r#"<a href="{}">{}</a>"#,
            escape(url.as_ref()),
            escape(label.as_ref())
        ))
    }

    pub fn custom_emoji(emoji_id: impl AsRef<str>, fallback: &str) -> Self {
        Self(format!(
            r#"<tg-emoji emoji-id="{}">{}</tg-emoji>"#,
            escape(emoji_id.as_ref()),
            escape(fallback)
        ))
    }

    pub fn push(&mut self, part: Html) {
        self.0.push_str(part.as_str());
    }

    pub fn line(&mut self, part: Html) {
        if !self.0.is_empty() {
            self.0.push('\n');
        }
        self.push(part);
    }

    pub fn blank_line(&mut self) {
        if !self.0.is_empty() {
            self.0.push_str("\n\n");
        }
    }

    pub fn into_string(self) -> String {
        self.0
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn bold(text: impl AsRef<str>) -> Html {
    Html::bold(text)
}

pub fn code(text: impl AsRef<str>) -> Html {
    Html::code(text)
}

pub fn link(label: impl AsRef<str>, url: impl AsRef<str>) -> Html {
    Html::link(label, url)
}

pub fn lines(parts: impl IntoIterator<Item = Html>) -> Html {
    let mut html = Html::empty();
    for part in parts {
        html.line(part);
    }
    html
}

pub fn paragraphs(parts: impl IntoIterator<Item = Html>) -> Html {
    let mut html = Html::empty();
    for part in parts {
        html.blank_line();
        html.push(part);
    }
    html
}

pub fn is_safe_len(html: &str) -> bool {
    html.chars().count() <= SAFE_TEXT_LIMIT
}

pub fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    text.chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>()
        + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_escapes_html() {
        assert_eq!(
            Html::text(r#"<b>"x" & y</b>"#).into_string(),
            "&lt;b&gt;&quot;x&quot; &amp; y&lt;/b&gt;"
        );
    }

    #[test]
    fn link_escapes_label_and_url() {
        assert_eq!(
            Html::link("<chat>", r#"https://example.com?a=1&b="x""#).into_string(),
            r#"<a href="https://example.com?a=1&amp;b=&quot;x&quot;">&lt;chat&gt;</a>"#
        );
    }

    #[test]
    fn code_and_bold_escape_content() {
        assert_eq!(Html::bold("<x>").into_string(), "<b>&lt;x&gt;</b>");
        assert_eq!(Html::code("<x>").into_string(), "<code>&lt;x&gt;</code>");
    }

    #[test]
    fn custom_emoji_escapes_id() {
        assert_eq!(
            Html::custom_emoji(r#"123"456"#, "😎").into_string(),
            r#"<tg-emoji emoji-id="123&quot;456">😎</tg-emoji>"#
        );
    }

    #[test]
    fn truncate_keeps_short_text_and_cuts_long_text() {
        assert_eq!(truncate_text("abc", 10), "abc");
        assert_eq!(truncate_text("abcdef", 4), "abc…");
    }
}
