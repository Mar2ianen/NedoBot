//! Текст rich-сообщений из Telegram Desktop export, чей формат отличается от Bot API.
//! Читаем только видимые поля известных блоков: file paths, IDs и callback data не включаются.
use serde_json::Value;

const MAX_DEPTH: usize = 64;

#[derive(Default)]
pub struct ExportRichProjection {
    pub text: String,
    pub has_links: bool,
}

pub fn project(value: &Value) -> ExportRichProjection {
    let mut projection = ExportRichProjection::default();
    projection.blocks(&value["blocks"], 0);
    projection.text = projection.text.trim().to_owned();
    projection.has_links |= ["http://", "https://", "t.me/"]
        .iter()
        .any(|prefix| projection.text.contains(prefix));
    projection
}

impl ExportRichProjection {
    fn inline(&mut self, value: &Value, depth: usize) {
        if depth >= MAX_DEPTH {
            return;
        }
        match value {
            Value::String(text) => self.text.push_str(text),
            Value::Array(parts) => {
                for part in parts {
                    self.inline(part, depth + 1);
                }
            }
            Value::Object(_) => match value["type"].as_str().unwrap_or_default() {
                "text_link" | "url" | "link" => {
                    self.has_links = true;
                    self.inline(&value["text"], depth + 1);
                }
                "plain" | "concat" | "bold" | "italic" | "underline" | "strikethrough"
                | "strike" | "spoiler" | "code" | "pre" | "custom_emoji" | "marked"
                | "subscript" | "superscript" | "text_mention" | "mention" | "hashtag"
                | "cashtag" | "bot_command" | "email" | "phone" | "datetime" => {
                    self.inline(&value["text"], depth + 1);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn caption(&mut self, value: &Value, depth: usize) {
        if value.is_object() {
            self.inline(&value["text"], depth + 1);
            if !value["credit"].is_null() {
                self.text.push('\n');
                self.inline(&value["credit"], depth + 1);
            }
        } else {
            self.inline(value, depth + 1);
        }
    }

    fn blocks(&mut self, value: &Value, depth: usize) {
        if depth >= MAX_DEPTH {
            return;
        }
        for block in value.as_array().into_iter().flatten() {
            match block["type"].as_str().unwrap_or_default() {
                "paragraph" | "heading" | "pre" | "footer" | "title" | "subtitle" => {
                    self.inline(&block["text"], depth + 1);
                }
                "quote" => {
                    if block["content"]
                        .as_array()
                        .is_some_and(|blocks| !blocks.is_empty())
                    {
                        self.blocks(&block["content"], depth + 1);
                    } else {
                        self.inline(&block["text"], depth + 1);
                    }
                    self.caption(&block["caption"], depth + 1);
                }
                "collage" | "slideshow" => {
                    self.blocks(&block["items"], depth + 1);
                    self.caption(&block["caption"], depth + 1);
                }
                "photo" | "video" | "audio" | "document" | "animation" => {
                    self.caption(&block["caption"], depth + 1);
                }
                _ => continue,
            }
            self.text.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn desktop_rich_projects_wrapped_text_captions_and_nested_quotes() {
        let value = json!({"blocks":[
            {"type":"heading","text":{"type":"plain","text":"Заголовок"}},
            {"type":"paragraph","text":{"type":"concat","text":[
                {"type":"bold","text":{"type":"plain","text":"Ссылка "}},
                {"type":"text_link","href":"https://example.com","text":{"type":"plain","text":"сюда"}},
                {"type":"custom_emoji","document_id":"private-id","text":"🙂"}
            ]}},
            {"type":"quote","text":{"type":"plain","text":"Не дублировать"},"content":[
                {"type":"paragraph","text":{"type":"plain","text":"Цитата"}}
            ],"caption":{"text":{"type":"plain","text":"Подпись"},"credit":{"type":"plain","text":"Автор"}}},
            {"type":"collage","items":[{"type":"photo","photo_id":"secret","photo_skip_reason":"local/file","caption":{"text":{"type":"plain","text":"Фото"}}}]},
            {"type":"thinking","text":"private reasoning"},
            {"type":"future_unknown","text":"unreviewed data"}
        ]});
        let projection = project(&value);
        assert!(projection.has_links);
        for text in [
            "Заголовок",
            "Ссылка сюда🙂",
            "Цитата",
            "Подпись",
            "Автор",
            "Фото",
        ] {
            assert!(projection.text.contains(text));
        }
        for private in [
            "private",
            "secret",
            "local/file",
            "unreviewed",
            "Не дублировать",
            "https://",
        ] {
            assert!(!projection.text.contains(private));
        }
    }
}
