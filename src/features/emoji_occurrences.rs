//! Ordered emoji occurrences from Telegram-authored message content.
//!
//! Offsets for ordinary text/captions are relative to the Telegram field and
//! therefore use Telegram's UTF-16 coordinates. Rich text offsets are relative
//! to the addressed leaf because Bot API rich blocks have no message-wide text
//! coordinate system. Reply, reaction, quote, and profile-status references are
//! kept separate from authored body occurrences.

use std::ops::Range;

use teloxide::types::{
    Message, MessageEntity, ReactionType, RichBlock, RichBlockKind, RichMessage, RichText,
    RichTextObject, TextQuote,
};

use crate::features::emoji_analysis::{
    self, CustomEmojiInput, EmojiAnalysisError, EmojiFormat, EmojiIdentity,
};
use crate::telegram::rich_content::rich_message;

const MAX_TRAVERSAL_NODES: usize = 8_192;
const MAX_TRAVERSAL_DEPTH: usize = 64;
const MAX_TRAVERSAL_PATH_BYTES: usize = 256 * 1_024;
const MAX_TOTAL_TEXT_BYTES: usize = 256 * 1_024;
const MAX_SERIALIZED_RICH_BYTES: usize = 512 * 1_024;
const MAX_TOTAL_OCCURRENCES: usize = 8_192;
const MAX_TOTAL_CUSTOM_EMOJI: usize = 2_048;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentSource {
    Text,
    Caption,
    RichMessage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmojiOccurrence {
    pub identity: EmojiIdentity,
    pub fallback: String,
    pub source: ContentSource,
    pub path: String,
    pub utf8_range: Range<usize>,
    pub utf16_range: Range<usize>,
    pub format: EmojiFormat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceKind {
    Reaction,
    Quote,
    Reply,
    ProfileStatus,
}

/// A separately scoped emoji reference that must not be counted as authored
/// message text. `fallback` is absent when Telegram supplies only an ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmojiReference {
    pub kind: ReferenceKind,
    pub identity: EmojiIdentity,
    pub fallback: Option<String>,
}

pub fn extract_message(message: &Message) -> Result<Vec<EmojiOccurrence>, EmojiAnalysisError> {
    let mut result = Vec::new();
    let mut aggregate_text_bytes = 0usize;
    if let Some(text) = message.text() {
        aggregate_text_bytes = text.len();
        if aggregate_text_bytes > MAX_TOTAL_TEXT_BYTES {
            return Err(EmojiAnalysisError::RichTraversalLimitExceeded);
        }
        append_telegram_text(
            text,
            message.entities().unwrap_or_default(),
            ContentSource::Text,
            "text",
            &mut result,
        )?;
    } else if let Some(caption) = message.caption() {
        aggregate_text_bytes = caption.len();
        if aggregate_text_bytes > MAX_TOTAL_TEXT_BYTES {
            return Err(EmojiAnalysisError::RichTraversalLimitExceeded);
        }
        append_telegram_text(
            caption,
            message.caption_entities().unwrap_or_default(),
            ContentSource::Caption,
            "caption",
            &mut result,
        )?;
    }

    if let Some(rich) = rich_message(message) {
        let rich_text_bytes = validate_rich_traversal_budget(rich)?;
        if aggregate_text_bytes.saturating_add(rich_text_bytes) > MAX_TOTAL_TEXT_BYTES {
            return Err(EmojiAnalysisError::RichTraversalLimitExceeded);
        }
        append_rich_message(rich, &mut result)?;
    }
    Ok(result)
}

fn append_telegram_text(
    text: &str,
    entities: &[MessageEntity],
    source: ContentSource,
    path: &str,
    output: &mut Vec<EmojiOccurrence>,
) -> Result<(), EmojiAnalysisError> {
    let analysis = emoji_analysis::analyze_telegram_text(text, entities)?;
    ensure_occurrence_budget(output, &analysis.occurrences)?;
    output.extend(
        analysis
            .occurrences
            .into_iter()
            .map(|item| EmojiOccurrence {
                identity: item.identity,
                fallback: item.fallback,
                source,
                path: path.to_owned(),
                utf8_range: item.utf8_range,
                utf16_range: item.utf16_range,
                format: item.format,
            }),
    );
    Ok(())
}

fn append_rich_message(
    rich: &RichMessage,
    output: &mut Vec<EmojiOccurrence>,
) -> Result<(), EmojiAnalysisError> {
    for (index, block) in rich.blocks.iter().enumerate() {
        append_block(block, &format!("rich_message.blocks[{index}]"), output)?;
    }
    Ok(())
}

fn validate_rich_traversal_budget(rich: &RichMessage) -> Result<usize, EmojiAnalysisError> {
    validate_serialized_budget(rich)
}

fn validate_serialized_budget(value: &impl serde::Serialize) -> Result<usize, EmojiAnalysisError> {
    let mut writer = BoundedJsonWriter::default();
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| EmojiAnalysisError::RichTraversalLimitExceeded)?;
    let value: serde_json::Value = serde_json::from_slice(&writer.0)
        .map_err(|_| EmojiAnalysisError::RichTraversalLimitExceeded)?;
    validate_value_budget(&value)
}

#[derive(Default)]
struct BoundedJsonWriter(Vec<u8>);

impl std::io::Write for BoundedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_SERIALIZED_RICH_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "rich message traversal budget exceeded",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn validate_value_budget(value: &serde_json::Value) -> Result<usize, EmojiAnalysisError> {
    let mut pending = vec![(value, 0usize, 0usize)];
    let (mut nodes, mut path_bytes, mut text_bytes) = (0usize, 0usize, 0usize);
    while let Some((node, depth, path_len)) = pending.pop() {
        nodes = nodes.saturating_add(1);
        path_bytes = path_bytes.saturating_add(path_len);
        if nodes > MAX_TRAVERSAL_NODES
            || depth > MAX_TRAVERSAL_DEPTH
            || path_bytes > MAX_TRAVERSAL_PATH_BYTES
        {
            return Err(EmojiAnalysisError::RichTraversalLimitExceeded);
        }
        match node {
            serde_json::Value::String(value) => {
                text_bytes = text_bytes.saturating_add(value.len());
                if text_bytes > MAX_TOTAL_TEXT_BYTES {
                    return Err(EmojiAnalysisError::RichTraversalLimitExceeded);
                }
            }
            serde_json::Value::Array(values) => {
                pending.extend(values.iter().map(|value| (value, depth + 1, 8)))
            }
            serde_json::Value::Object(values) => pending.extend(
                values
                    .iter()
                    .map(|(key, value)| (value, depth + 1, key.len().saturating_add(1))),
            ),
            _ => {}
        }
    }
    Ok(text_bytes)
}

fn ensure_occurrence_budget(
    output: &[EmojiOccurrence],
    additions: &[emoji_analysis::EmojiOccurrence],
) -> Result<(), EmojiAnalysisError> {
    if output.len().saturating_add(additions.len()) > MAX_TOTAL_OCCURRENCES {
        return Err(EmojiAnalysisError::RichTraversalLimitExceeded);
    }
    let existing_custom = output
        .iter()
        .filter(|item| matches!(item.identity, EmojiIdentity::Custom(_)))
        .count();
    let new_custom = additions
        .iter()
        .filter(|item| matches!(item.identity, EmojiIdentity::Custom(_)))
        .count();
    if existing_custom.saturating_add(new_custom) > MAX_TOTAL_CUSTOM_EMOJI {
        return Err(EmojiAnalysisError::RichTraversalLimitExceeded);
    }
    Ok(())
}

fn append_block(
    block: &RichBlock,
    path: &str,
    output: &mut Vec<EmojiOccurrence>,
) -> Result<(), EmojiAnalysisError> {
    let RichBlock::Known(block) = block else {
        return Ok(());
    };
    match block.as_ref() {
        RichBlockKind::Paragraph(v) => {
            append_inline(&v.text, &format!("{path}.paragraph.text"), output)?
        }
        RichBlockKind::Heading(v) => {
            append_inline(&v.text, &format!("{path}.heading.text"), output)?
        }
        RichBlockKind::Pre(v) => append_inline(&v.text, &format!("{path}.pre.text"), output)?,
        RichBlockKind::Footer(v) => append_inline(&v.text, &format!("{path}.footer.text"), output)?,
        RichBlockKind::MathematicalExpression(v) => append_leaf(
            &v.expression,
            None,
            &format!("{path}.math.expression"),
            output,
        )?,
        RichBlockKind::List(v) => {
            for (item_index, item) in v.items.iter().enumerate() {
                let item_path = format!("{path}.list.items[{item_index}]");
                append_leaf(&item.label, None, &format!("{item_path}.label"), output)?;
                for (child_index, child) in item.blocks.iter().enumerate() {
                    append_block(child, &format!("{item_path}.blocks[{child_index}]"), output)?;
                }
            }
        }
        RichBlockKind::Blockquote(v) => {
            append_blocks(&v.blocks, &format!("{path}.blockquote.blocks"), output)?;
            append_optional_inline(&v.credit, &format!("{path}.blockquote.credit"), output)?;
        }
        RichBlockKind::ExpandableBlockquote(v) => {
            append_blocks(
                &v.blocks,
                &format!("{path}.expandable_blockquote.blocks"),
                output,
            )?;
            append_optional_inline(
                &v.credit,
                &format!("{path}.expandable_blockquote.credit"),
                output,
            )?;
        }
        RichBlockKind::Pullquote(v) => {
            append_inline(&v.text, &format!("{path}.pullquote.text"), output)?;
            append_optional_inline(&v.credit, &format!("{path}.pullquote.credit"), output)?;
        }
        RichBlockKind::Collage(v) => {
            append_blocks(&v.blocks, &format!("{path}.collage.blocks"), output)?;
            append_caption(&v.caption, &format!("{path}.collage.caption"), output)?;
        }
        RichBlockKind::Slideshow(v) => {
            append_blocks(&v.blocks, &format!("{path}.slideshow.blocks"), output)?;
            append_caption(&v.caption, &format!("{path}.slideshow.caption"), output)?;
        }
        RichBlockKind::Table(v) => {
            for (row_index, row) in v.cells.iter().enumerate() {
                for (cell_index, cell) in row.iter().enumerate() {
                    if let Some(text) = &cell.text {
                        append_inline(
                            text,
                            &format!("{path}.table.cells[{row_index}][{cell_index}].text"),
                            output,
                        )?;
                    }
                }
            }
            append_optional_inline(&v.caption, &format!("{path}.table.caption"), output)?;
        }
        RichBlockKind::Buttons(v) => {
            for (index, button) in v.buttons.iter().enumerate() {
                append_inline(
                    &button.text,
                    &format!("{path}.buttons[{index}].text"),
                    output,
                )?;
            }
        }
        RichBlockKind::Details(v) => {
            append_inline(&v.summary, &format!("{path}.details.summary"), output)?;
            append_blocks(&v.blocks, &format!("{path}.details.blocks"), output)?;
        }
        RichBlockKind::Document(v) => {
            append_caption(&v.caption, &format!("{path}.document.caption"), output)?
        }
        RichBlockKind::Map(v) => {
            append_caption(&v.caption, &format!("{path}.map.caption"), output)?
        }
        RichBlockKind::Animation(v) => {
            append_caption(&v.caption, &format!("{path}.animation.caption"), output)?
        }
        RichBlockKind::Audio(v) => {
            append_caption(&v.caption, &format!("{path}.audio.caption"), output)?
        }
        RichBlockKind::Photo(v) => {
            append_caption(&v.caption, &format!("{path}.photo.caption"), output)?
        }
        RichBlockKind::Video(v) => {
            append_caption(&v.caption, &format!("{path}.video.caption"), output)?
        }
        RichBlockKind::VoiceNote(v) => {
            append_caption(&v.caption, &format!("{path}.voice_note.caption"), output)?
        }
        RichBlockKind::Divider(_) | RichBlockKind::Anchor(_) | RichBlockKind::Thinking(_) => {}
    }
    Ok(())
}

fn append_blocks(
    blocks: &[RichBlock],
    path: &str,
    output: &mut Vec<EmojiOccurrence>,
) -> Result<(), EmojiAnalysisError> {
    for (index, block) in blocks.iter().enumerate() {
        append_block(block, &format!("{path}[{index}]"), output)?;
    }
    Ok(())
}

fn append_caption(
    caption: &Option<teloxide::types::RichBlockCaption>,
    path: &str,
    output: &mut Vec<EmojiOccurrence>,
) -> Result<(), EmojiAnalysisError> {
    if let Some(caption) = caption {
        append_inline(&caption.text, &format!("{path}.text"), output)?;
        append_optional_inline(&caption.credit, &format!("{path}.credit"), output)?;
    }
    Ok(())
}

fn append_optional_inline(
    text: &Option<RichText>,
    path: &str,
    output: &mut Vec<EmojiOccurrence>,
) -> Result<(), EmojiAnalysisError> {
    if let Some(text) = text {
        append_inline(text, path, output)?;
    }
    Ok(())
}

fn append_inline(
    text: &RichText,
    path: &str,
    output: &mut Vec<EmojiOccurrence>,
) -> Result<(), EmojiAnalysisError> {
    match text {
        RichText::Text(text) => append_leaf(text, None, path, output)?,
        RichText::List(items) => {
            for (index, item) in items.iter().enumerate() {
                append_inline(item, &format!("{path}[{index}]"), output)?;
            }
        }
        RichText::Object(object) => match object {
            RichTextObject::Bold(v) => append_inline(&v.text, &format!("{path}.bold"), output)?,
            RichTextObject::Italic(v) => append_inline(&v.text, &format!("{path}.italic"), output)?,
            RichTextObject::Underline(v) => {
                append_inline(&v.text, &format!("{path}.underline"), output)?
            }
            RichTextObject::Strikethrough(v) => {
                append_inline(&v.text, &format!("{path}.strikethrough"), output)?
            }
            RichTextObject::Spoiler(v) => {
                append_inline(&v.text, &format!("{path}.spoiler"), output)?
            }
            RichTextObject::DateTime(v) => {
                append_inline(&v.text, &format!("{path}.datetime"), output)?
            }
            RichTextObject::TextMention(v) => {
                append_inline(&v.text, &format!("{path}.text_mention"), output)?
            }
            RichTextObject::Subscript(v) => {
                append_inline(&v.text, &format!("{path}.subscript"), output)?
            }
            RichTextObject::Superscript(v) => {
                append_inline(&v.text, &format!("{path}.superscript"), output)?
            }
            RichTextObject::Marked(v) => append_inline(&v.text, &format!("{path}.marked"), output)?,
            RichTextObject::Code(v) => append_inline(&v.text, &format!("{path}.code"), output)?,
            RichTextObject::CustomEmoji(v) => append_leaf(
                &v.alternative_text,
                Some(v.custom_emoji_id.to_string()),
                &format!("{path}.custom_emoji"),
                output,
            )?,
            RichTextObject::MathematicalExpression(v) => {
                append_leaf(&v.expression, None, &format!("{path}.math"), output)?
            }
            RichTextObject::Url(v) => append_inline(&v.text, &format!("{path}.url.text"), output)?,
            RichTextObject::EmailAddress(v) => {
                append_inline(&v.text, &format!("{path}.email.text"), output)?
            }
            RichTextObject::PhoneNumber(v) => {
                append_inline(&v.text, &format!("{path}.phone.text"), output)?
            }
            RichTextObject::BankCardNumber(v) => {
                append_inline(&v.text, &format!("{path}.bank_card.text"), output)?
            }
            RichTextObject::TonAddress(v) => {
                append_inline(&v.text, &format!("{path}.ton_address.text"), output)?
            }
            RichTextObject::Mention(v) => {
                append_inline(&v.text, &format!("{path}.mention.text"), output)?
            }
            RichTextObject::Hashtag(v) => {
                append_inline(&v.text, &format!("{path}.hashtag.text"), output)?
            }
            RichTextObject::Cashtag(v) => {
                append_inline(&v.text, &format!("{path}.cashtag.text"), output)?
            }
            RichTextObject::BotCommand(v) => {
                append_inline(&v.text, &format!("{path}.bot_command.text"), output)?
            }
            RichTextObject::Button(v) => {
                append_inline(&v.button.text, &format!("{path}.button.text"), output)?
            }
            RichTextObject::AnchorLink(v) => {
                append_inline(&v.text, &format!("{path}.anchor_link.text"), output)?
            }
            RichTextObject::Reference(v) => {
                append_inline(&v.text, &format!("{path}.reference.text"), output)?
            }
            RichTextObject::ReferenceLink(v) => {
                append_inline(&v.text, &format!("{path}.reference_link.text"), output)?
            }
            RichTextObject::Anchor(_) | RichTextObject::Unknown(_) => {}
        },
    }
    Ok(())
}

fn append_leaf(
    text: &str,
    custom_id: Option<String>,
    path: &str,
    output: &mut Vec<EmojiOccurrence>,
) -> Result<(), EmojiAnalysisError> {
    let custom = custom_id
        .map(|id| CustomEmojiInput {
            id,
            utf16_range: 0..text.encode_utf16().count(),
            format: EmojiFormat::Unknown,
        })
        .into_iter()
        .collect::<Vec<_>>();
    let analysis = emoji_analysis::analyze(text, &custom)?;
    ensure_occurrence_budget(output, &analysis.occurrences)?;
    output.extend(
        analysis
            .occurrences
            .into_iter()
            .map(|item| EmojiOccurrence {
                identity: item.identity,
                fallback: item.fallback,
                source: ContentSource::RichMessage,
                path: path.to_owned(),
                utf8_range: item.utf8_range,
                utf16_range: item.utf16_range,
                format: item.format,
            }),
    );
    Ok(())
}

pub fn reaction_references(reactions: &[ReactionType]) -> Vec<EmojiReference> {
    reactions
        .iter()
        .filter_map(|reaction| match reaction {
            ReactionType::Emoji { emoji } => Some(EmojiReference {
                kind: ReferenceKind::Reaction,
                identity: EmojiIdentity::Unicode(emoji.clone()),
                fallback: Some(emoji.clone()),
            }),
            ReactionType::CustomEmoji { custom_emoji_id } => Some(EmojiReference {
                kind: ReferenceKind::Reaction,
                identity: EmojiIdentity::Custom(custom_emoji_id.to_string()),
                fallback: None,
            }),
            ReactionType::Paid => None,
        })
        .collect()
}

pub fn quote_references(quote: &TextQuote) -> Result<Vec<EmojiReference>, EmojiAnalysisError> {
    let mut references = Vec::new();
    if let Some(rich_texts) = quote.rich_texts.as_ref().filter(|texts| !texts.is_empty()) {
        validate_serialized_budget(rich_texts)?;
        let mut occurrences = Vec::new();
        for (index, text) in rich_texts.iter().enumerate() {
            append_inline(
                text,
                &format!("quote.rich_texts[{index}]"),
                &mut occurrences,
            )?;
        }
        references.extend(occurrences.into_iter().map(|item| EmojiReference {
            kind: ReferenceKind::Quote,
            identity: item.identity,
            fallback: Some(item.fallback),
        }));
    } else {
        let analysis = emoji_analysis::analyze_telegram_text(&quote.text, &quote.entities)?;
        ensure_occurrence_budget(&[], &analysis.occurrences)?;
        references.extend(analysis.occurrences.into_iter().map(|item| EmojiReference {
            kind: ReferenceKind::Quote,
            identity: item.identity,
            fallback: Some(item.fallback),
        }));
    }
    Ok(references)
}

pub fn chat_status_reference(id: Option<&str>) -> Option<EmojiReference> {
    id.map(|id| EmojiReference {
        kind: ReferenceKind::ProfileStatus,
        identity: EmojiIdentity::Custom(id.to_owned()),
        fallback: None,
    })
}

pub fn custom_emoji_reference(
    kind: ReferenceKind,
    id: impl ToString,
    fallback: Option<String>,
) -> EmojiReference {
    EmojiReference {
        kind,
        identity: EmojiIdentity::Custom(id.to_string()),
        fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn typed_message_extraction_preserves_caption_and_custom_fallback_identity() {
        let message: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": 1, "type": "private"},
            "photo": [{"file_id":"file", "file_unique_id":"unique", "width":1,"height":1}],
            "caption": "A 😀 B",
            "caption_entities": [{"type":"custom_emoji", "offset":2,"length":2,"custom_emoji_id":"42"}]
        })).unwrap();
        let found = extract_message(&message).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].identity, EmojiIdentity::Custom("42".into()));
        assert_eq!(found[0].fallback, "😀");
        assert_eq!(found[0].source, ContentSource::Caption);
        assert_eq!(found[0].utf16_range, 2..4);
    }

    #[test]
    fn rich_budget_rejects_excess_nodes_depth_paths_and_total_text() {
        let too_many_nodes = serde_json::Value::Array(vec![json!(null); MAX_TRAVERSAL_NODES]);
        assert_eq!(
            validate_value_budget(&too_many_nodes),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );

        let mut too_deep = json!(null);
        for _ in 0..=MAX_TRAVERSAL_DEPTH {
            too_deep = json!([too_deep]);
        }
        assert_eq!(
            validate_value_budget(&too_deep),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );

        let too_much_text = json!("x".repeat(MAX_TOTAL_TEXT_BYTES + 1));
        assert_eq!(
            validate_value_budget(&too_much_text),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );
        assert_eq!(
            validate_serialized_budget(&"x".repeat(MAX_SERIALIZED_RICH_BYTES + 1)),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );

        let mut too_many_path_bytes = serde_json::Map::new();
        for index in 0..512 {
            too_many_path_bytes.insert(format!("{}-{index}", "k".repeat(600)), json!(null));
        }
        assert_eq!(
            validate_value_budget(&serde_json::Value::Object(too_many_path_bytes)),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );
    }

    #[test]
    fn occurrence_budget_counts_across_independent_leaves() {
        let mut output = vec![fake_occurrence(); MAX_TOTAL_OCCURRENCES];
        let addition = emoji_analysis::analyze("😀", &[]).unwrap().occurrences;
        assert_eq!(
            ensure_occurrence_budget(&output, &addition),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );
        output.clear();
        let custom = emoji_analysis::analyze(
            "x",
            &[CustomEmojiInput {
                id: "id".into(),
                utf16_range: 0..1,
                format: EmojiFormat::Unknown,
            }],
        )
        .unwrap()
        .occurrences;
        output.resize_with(MAX_TOTAL_CUSTOM_EMOJI, fake_custom_occurrence);
        assert_eq!(
            ensure_occurrence_budget(&output, &custom),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );
    }

    fn fake_occurrence() -> EmojiOccurrence {
        EmojiOccurrence {
            identity: EmojiIdentity::Unicode("😀".into()),
            fallback: "😀".into(),
            source: ContentSource::Text,
            path: "text".into(),
            utf8_range: 0..4,
            utf16_range: 0..2,
            format: EmojiFormat::Unicode,
        }
    }

    fn fake_custom_occurrence() -> EmojiOccurrence {
        EmojiOccurrence {
            identity: EmojiIdentity::Custom("id".into()),
            ..fake_occurrence()
        }
    }

    #[test]
    fn rich_nested_paths_keep_custom_and_unicode_occurrences_separate() {
        let rich: RichMessage = serde_json::from_value(json!({"blocks":[
            {"type":"paragraph", "text":{"type":"bold", "text":["😀", {"type":"custom_emoji", "custom_emoji_id":"99", "alternative_text":"😎"}]}}
        ]})).unwrap();
        let mut found = Vec::new();
        append_rich_message(&rich, &mut found).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].identity, EmojiIdentity::Unicode("😀".into()));
        assert!(found[0].path.contains("paragraph.text.bold"));
        assert_eq!(found[1].identity, EmojiIdentity::Custom("99".into()));
        assert_eq!(found[1].fallback, "😎");
    }

    #[test]
    fn reactions_are_references_and_not_authored_body_occurrences() {
        let refs = reaction_references(&[
            ReactionType::Emoji {
                emoji: "🔥".into()
            },
            ReactionType::CustomEmoji {
                custom_emoji_id: "77".into(),
            },
            ReactionType::Paid,
        ]);
        assert_eq!(refs.len(), 2);
        assert!(refs.iter().all(|item| item.kind == ReferenceKind::Reaction));
        assert_eq!(refs[1].fallback, None);
    }

    #[test]
    fn quote_entities_and_chat_status_remain_separate_references() {
        let quote = TextQuote {
            text: "😀".into(),
            entities: vec![MessageEntity::custom_emoji("99".into(), 0, 2)],
            rich_texts: None,
            position: 0,
            is_manual: true,
        };
        let refs = quote_references(&quote).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, ReferenceKind::Quote);
        assert_eq!(refs[0].identity, EmojiIdentity::Custom("99".into()));
        assert_eq!(refs[0].fallback.as_deref(), Some("😀"));

        let rich_quote = TextQuote {
            text: "✨".into(),
            entities: Vec::new(),
            rich_texts: Some(vec![RichText::Object(RichTextObject::CustomEmoji(
                serde_json::from_value(json!({
                    "type": "custom_emoji",
                    "custom_emoji_id": "100",
                    "alternative_text": "✨"
                }))
                .unwrap(),
            ))]),
            position: 0,
            is_manual: true,
        };
        let rich_refs = quote_references(&rich_quote).unwrap();
        assert_eq!(rich_refs.len(), 1, "rich projection must not double count");
        assert_eq!(rich_refs[0].identity, EmojiIdentity::Custom("100".into()));
        assert_eq!(rich_refs[0].fallback.as_deref(), Some("✨"));

        let status = chat_status_reference(Some("status-id")).unwrap();
        assert_eq!(status.kind, ReferenceKind::ProfileStatus);
        assert_eq!(status.identity, EmojiIdentity::Custom("status-id".into()));
    }
}
