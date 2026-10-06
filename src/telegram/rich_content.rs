//! Текстовая проекция typed RichMessage для истории, reply-контекста и модерации.
//! File IDs, callback data и скрытые thinking-блоки в проекцию не попадают.
use teloxide::types::{
    MediaKind, Message, MessageKind, RichBlock, RichBlockCaption, RichBlockKind, RichMessage,
    RichText, RichTextObject,
};

#[derive(Default)]
pub(super) struct RichProjection {
    pub text: String,
    pub custom_emoji_ids: Vec<String>,
    pub has_links: bool,
}

pub fn rich_message(message: &Message) -> Option<&RichMessage> {
    let MessageKind::Common(common) = &message.kind else {
        return None;
    };
    let MediaKind::RichMessage(media) = &common.media_kind else {
        return None;
    };
    Some(&media.rich_message)
}

pub(super) fn project(message: &RichMessage) -> RichProjection {
    let mut projection = RichProjection::default();
    projection.blocks(&message.blocks);
    projection.text = projection.text.trim().to_owned();
    projection
}

impl RichProjection {
    fn inline(&mut self, text: &RichText) {
        match text {
            RichText::Text(text) => self.text.push_str(text),
            RichText::List(items) => items.iter().for_each(|item| self.inline(item)),
            RichText::Object(object) => match object {
                RichTextObject::Bold(v) => self.inline(&v.text),
                RichTextObject::Italic(v) => self.inline(&v.text),
                RichTextObject::Underline(v) => self.inline(&v.text),
                RichTextObject::Strikethrough(v) => self.inline(&v.text),
                RichTextObject::Spoiler(v) => self.inline(&v.text),
                RichTextObject::DateTime(v) => self.inline(&v.text),
                RichTextObject::TextMention(v) => self.inline(&v.text),
                RichTextObject::Subscript(v) => self.inline(&v.text),
                RichTextObject::Superscript(v) => self.inline(&v.text),
                RichTextObject::Marked(v) => self.inline(&v.text),
                RichTextObject::Code(v) => self.inline(&v.text),
                RichTextObject::CustomEmoji(v) => {
                    self.text.push_str(&v.alternative_text);
                    self.custom_emoji_ids.push(v.custom_emoji_id.clone());
                }
                RichTextObject::MathematicalExpression(v) => self.text.push_str(&v.expression),
                RichTextObject::Url(v) => {
                    self.has_links = true;
                    self.inline(&v.text);
                }
                RichTextObject::EmailAddress(v) => self.inline(&v.text),
                RichTextObject::PhoneNumber(v) => self.inline(&v.text),
                RichTextObject::BankCardNumber(v) => self.inline(&v.text),
                RichTextObject::Mention(v) => self.inline(&v.text),
                RichTextObject::Hashtag(v) => self.inline(&v.text),
                RichTextObject::Cashtag(v) => self.inline(&v.text),
                RichTextObject::BotCommand(v) => self.inline(&v.text),
                RichTextObject::Button(v) => {
                    self.has_links |= v.button.url.is_some();
                    self.inline(&v.button.text);
                }
                RichTextObject::AnchorLink(v) => self.inline(&v.text),
                RichTextObject::Reference(v) => self.inline(&v.text),
                RichTextObject::ReferenceLink(v) => self.inline(&v.text),
                RichTextObject::Anchor(_) | RichTextObject::Unknown(_) => {}
            },
        }
    }

    fn caption(&mut self, caption: &Option<RichBlockCaption>) {
        if let Some(caption) = caption {
            self.inline(&caption.text);
            self.credit(&caption.credit);
        }
    }

    fn credit(&mut self, text: &Option<RichText>) {
        if let Some(text) = text {
            self.text.push('\n');
            self.inline(text);
        }
    }

    fn blocks(&mut self, blocks: &[RichBlock]) {
        for block in blocks {
            let RichBlock::Known(block) = block else {
                continue;
            };
            match block.as_ref() {
                RichBlockKind::Paragraph(v) => self.inline(&v.text),
                RichBlockKind::Heading(v) => self.inline(&v.text),
                RichBlockKind::Pre(v) => self.inline(&v.text),
                RichBlockKind::Footer(v) => self.inline(&v.text),
                RichBlockKind::MathematicalExpression(v) => self.text.push_str(&v.expression),
                RichBlockKind::List(v) => {
                    for item in &v.items {
                        self.text.push_str(&item.label);
                        self.text.push(' ');
                        self.blocks(&item.blocks);
                    }
                }
                RichBlockKind::Blockquote(v) => {
                    self.blocks(&v.blocks);
                    self.credit(&v.credit);
                }
                RichBlockKind::ExpandableBlockquote(v) => {
                    self.inline(&v.text);
                    self.credit(&v.credit);
                }
                RichBlockKind::Pullquote(v) => {
                    self.inline(&v.text);
                    self.credit(&v.credit);
                }
                RichBlockKind::Collage(v) => {
                    self.blocks(&v.blocks);
                    self.caption(&v.caption);
                }
                RichBlockKind::Slideshow(v) => {
                    self.blocks(&v.blocks);
                    self.caption(&v.caption);
                }
                RichBlockKind::Table(v) => {
                    for row in &v.cells {
                        for (index, cell) in row.iter().enumerate() {
                            if index > 0 {
                                self.text.push('\t');
                            }
                            if let Some(text) = &cell.text {
                                self.inline(text);
                            }
                        }
                        self.text.push('\n');
                    }
                    self.credit(&v.caption);
                }
                RichBlockKind::Buttons(v) => {
                    for button in &v.buttons {
                        self.has_links |= button.url.is_some();
                        self.inline(&button.text);
                        self.text.push('\n');
                    }
                }
                RichBlockKind::Details(v) => {
                    self.inline(&v.summary);
                    self.text.push('\n');
                    self.blocks(&v.blocks);
                }
                RichBlockKind::Document(v) => self.caption(&v.caption),
                RichBlockKind::Map(v) => self.caption(&v.caption),
                RichBlockKind::Animation(v) => self.caption(&v.caption),
                RichBlockKind::Audio(v) => self.caption(&v.caption),
                RichBlockKind::Photo(v) => self.caption(&v.caption),
                RichBlockKind::Video(v) => self.caption(&v.caption),
                RichBlockKind::VoiceNote(v) => self.caption(&v.caption),
                RichBlockKind::Divider(_)
                | RichBlockKind::Anchor(_)
                | RichBlockKind::Thinking(_) => continue,
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
    fn projects_nested_text_links_emoji_and_tables_without_private_metadata() {
        let rich: RichMessage = serde_json::from_value(json!({"blocks": [
            {"type":"heading","size":1,"text":{"type":"bold","text":"Ответ"}},
            {"type":"paragraph","text":["Найдено ",{"type":"url","url":"https://example.org","text":"82 сообщения"},
                {"type":"custom_emoji","custom_emoji_id":"123","alternative_text":"🙂"}]},
            {"type":"details","summary":"Контекст","blocks":[{"type":"paragraph","text":"Предыдущий вопрос"}]},
            {"type":"table","cells":[[{"text":"Автор","align":"left","valign":"top"},{"text":"Текст","align":"left","valign":"top"}]]},
            {"type":"thinking","text":"private reasoning"},
            {"type":"future_block","file_id":"secret","text":"unknown content"}
        ]})).unwrap();
        let projected = project(&rich);
        assert_eq!(
            projected.text,
            "Ответ\nНайдено 82 сообщения🙂\nКонтекст\nПредыдущий вопрос\n\nАвтор\tТекст"
        );
        assert_eq!(projected.custom_emoji_ids, ["123"]);
        assert!(projected.has_links);
        assert!(!projected.text.contains("private"));
        assert!(!projected.text.contains("secret"));
    }
}
