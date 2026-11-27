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
const MAX_SINGLE_PATH_BYTES: usize = 4_096;
const MAX_TOTAL_TEXT_BYTES: usize = 256 * 1_024;
const MAX_TOTAL_OCCURRENCES: usize = 8_192;
const MAX_TOTAL_CUSTOM_EMOJI: usize = 2_048;

#[derive(Default)]
struct TraversalBudget {
    nodes: usize,
    path_bytes: usize,
    text_bytes: usize,
    occurrences: usize,
    custom_emoji: usize,
}

impl TraversalBudget {
    fn visit(&mut self, path: &str, depth: usize) -> Result<(), EmojiAnalysisError> {
        self.nodes = self.nodes.saturating_add(1);
        self.path_bytes = self.path_bytes.saturating_add(path.len());
        if self.nodes > MAX_TRAVERSAL_NODES
            || depth > MAX_TRAVERSAL_DEPTH
            || path.len() > MAX_SINGLE_PATH_BYTES
            || self.path_bytes > MAX_TRAVERSAL_PATH_BYTES
        {
            return Err(EmojiAnalysisError::RichTraversalLimitExceeded);
        }
        Ok(())
    }

    fn charge_text(&mut self, text: &str) -> Result<(), EmojiAnalysisError> {
        self.text_bytes = self.text_bytes.saturating_add(text.len());
        if self.text_bytes > MAX_TOTAL_TEXT_BYTES {
            return Err(EmojiAnalysisError::RichTraversalLimitExceeded);
        }
        Ok(())
    }

    fn charge_analysis(
        &mut self,
        analysis: &emoji_analysis::EmojiAnalysis,
    ) -> Result<(), EmojiAnalysisError> {
        self.occurrences = self.occurrences.saturating_add(analysis.occurrences.len());
        self.custom_emoji = self
            .custom_emoji
            .saturating_add(analysis.metrics.custom_count as usize);
        if self.occurrences > MAX_TOTAL_OCCURRENCES || self.custom_emoji > MAX_TOTAL_CUSTOM_EMOJI {
            return Err(EmojiAnalysisError::RichTraversalLimitExceeded);
        }
        Ok(())
    }
}

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
    let mut budget = TraversalBudget::default();
    if let Some(text) = message.text() {
        append_telegram_text(
            text,
            message.entities().unwrap_or_default(),
            ContentSource::Text,
            "text",
            &mut result,
            &mut budget,
        )?;
    } else if let Some(caption) = message.caption() {
        append_telegram_text(
            caption,
            message.caption_entities().unwrap_or_default(),
            ContentSource::Caption,
            "caption",
            &mut result,
            &mut budget,
        )?;
    }

    if let Some(rich) = rich_message(message) {
        append_rich_message(rich, &mut result, &mut budget)?;
    }
    Ok(result)
}

fn append_telegram_text(
    text: &str,
    entities: &[MessageEntity],
    source: ContentSource,
    path: &str,
    output: &mut Vec<EmojiOccurrence>,
    budget: &mut TraversalBudget,
) -> Result<(), EmojiAnalysisError> {
    budget.visit(path, 0)?;
    budget.charge_text(text)?;
    let analysis = emoji_analysis::analyze_telegram_text(text, entities)?;
    budget.charge_analysis(&analysis)?;
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
    budget: &mut TraversalBudget,
) -> Result<(), EmojiAnalysisError> {
    budget.visit("rich_message", 0)?;
    for (index, block) in rich.blocks.iter().enumerate() {
        append_block(
            block,
            &format!("rich_message.blocks[{index}]"),
            output,
            budget,
            1,
        )?;
    }
    Ok(())
}

fn append_block(
    block: &RichBlock,
    path: &str,
    output: &mut Vec<EmojiOccurrence>,
    budget: &mut TraversalBudget,
    depth: usize,
) -> Result<(), EmojiAnalysisError> {
    budget.visit(path, depth)?;
    let RichBlock::Known(block) = block else {
        return Ok(());
    };
    match block.as_ref() {
        RichBlockKind::Paragraph(v) => append_inline(
            &v.text,
            &format!("{path}.paragraph.text"),
            output,
            budget,
            depth + 1,
        )?,
        RichBlockKind::Heading(v) => append_inline(
            &v.text,
            &format!("{path}.heading.text"),
            output,
            budget,
            depth + 1,
        )?,
        RichBlockKind::Pre(v) => append_inline(
            &v.text,
            &format!("{path}.pre.text"),
            output,
            budget,
            depth + 1,
        )?,
        RichBlockKind::Footer(v) => append_inline(
            &v.text,
            &format!("{path}.footer.text"),
            output,
            budget,
            depth + 1,
        )?,
        RichBlockKind::MathematicalExpression(v) => append_leaf(
            &v.expression,
            None,
            &format!("{path}.math.expression"),
            output,
            budget,
            depth + 1,
        )?,
        RichBlockKind::List(v) => {
            for (item_index, item) in v.items.iter().enumerate() {
                let item_path = format!("{path}.list.items[{item_index}]");
                budget.visit(&item_path, depth + 1)?;
                append_leaf(
                    &item.label,
                    None,
                    &format!("{item_path}.label"),
                    output,
                    budget,
                    depth + 1,
                )?;
                for (child_index, child) in item.blocks.iter().enumerate() {
                    append_block(
                        child,
                        &format!("{item_path}.blocks[{child_index}]"),
                        output,
                        budget,
                        depth + 1,
                    )?;
                }
            }
        }
        RichBlockKind::Blockquote(v) => {
            append_blocks(
                &v.blocks,
                &format!("{path}.blockquote.blocks"),
                output,
                budget,
                depth + 1,
            )?;
            append_optional_inline(
                &v.credit,
                &format!("{path}.blockquote.credit"),
                output,
                budget,
                depth + 1,
            )?;
        }
        RichBlockKind::ExpandableBlockquote(v) => {
            append_blocks(
                &v.blocks,
                &format!("{path}.expandable_blockquote.blocks"),
                output,
                budget,
                depth + 1,
            )?;
            append_optional_inline(
                &v.credit,
                &format!("{path}.expandable_blockquote.credit"),
                output,
                budget,
                depth + 1,
            )?;
        }
        RichBlockKind::Pullquote(v) => {
            append_inline(
                &v.text,
                &format!("{path}.pullquote.text"),
                output,
                budget,
                depth + 1,
            )?;
            append_optional_inline(
                &v.credit,
                &format!("{path}.pullquote.credit"),
                output,
                budget,
                depth + 1,
            )?;
        }
        RichBlockKind::Collage(v) => {
            append_blocks(
                &v.blocks,
                &format!("{path}.collage.blocks"),
                output,
                budget,
                depth + 1,
            )?;
            append_caption(
                &v.caption,
                &format!("{path}.collage.caption"),
                output,
                budget,
                depth + 1,
            )?;
        }
        RichBlockKind::Slideshow(v) => {
            append_blocks(
                &v.blocks,
                &format!("{path}.slideshow.blocks"),
                output,
                budget,
                depth + 1,
            )?;
            append_caption(
                &v.caption,
                &format!("{path}.slideshow.caption"),
                output,
                budget,
                depth + 1,
            )?;
        }
        RichBlockKind::Table(v) => {
            for (row_index, row) in v.cells.iter().enumerate() {
                budget.visit(&format!("{path}.table.rows[{row_index}]"), depth + 1)?;
                for (cell_index, cell) in row.iter().enumerate() {
                    let cell_path = format!("{path}.table.cells[{row_index}][{cell_index}]");
                    budget.visit(&cell_path, depth + 2)?;
                    if let Some(text) = &cell.text {
                        append_inline(
                            text,
                            &format!("{path}.table.cells[{row_index}][{cell_index}].text"),
                            output,
                            budget,
                            depth + 3,
                        )?;
                    }
                }
            }
            append_optional_inline(
                &v.caption,
                &format!("{path}.table.caption"),
                output,
                budget,
                depth + 1,
            )?;
        }
        RichBlockKind::Buttons(v) => {
            for (index, button) in v.buttons.iter().enumerate() {
                budget.visit(&format!("{path}.buttons[{index}]"), depth + 1)?;
                append_inline(
                    &button.text,
                    &format!("{path}.buttons[{index}].text"),
                    output,
                    budget,
                    depth + 1,
                )?;
            }
        }
        RichBlockKind::Details(v) => {
            append_inline(
                &v.summary,
                &format!("{path}.details.summary"),
                output,
                budget,
                depth + 1,
            )?;
            append_blocks(
                &v.blocks,
                &format!("{path}.details.blocks"),
                output,
                budget,
                depth + 1,
            )?;
        }
        RichBlockKind::Document(v) => append_caption(
            &v.caption,
            &format!("{path}.document.caption"),
            output,
            budget,
            depth + 1,
        )?,
        RichBlockKind::Map(v) => append_caption(
            &v.caption,
            &format!("{path}.map.caption"),
            output,
            budget,
            depth + 1,
        )?,
        RichBlockKind::Animation(v) => append_caption(
            &v.caption,
            &format!("{path}.animation.caption"),
            output,
            budget,
            depth + 1,
        )?,
        RichBlockKind::Audio(v) => append_caption(
            &v.caption,
            &format!("{path}.audio.caption"),
            output,
            budget,
            depth + 1,
        )?,
        RichBlockKind::Photo(v) => append_caption(
            &v.caption,
            &format!("{path}.photo.caption"),
            output,
            budget,
            depth + 1,
        )?,
        RichBlockKind::Video(v) => append_caption(
            &v.caption,
            &format!("{path}.video.caption"),
            output,
            budget,
            depth + 1,
        )?,
        RichBlockKind::VoiceNote(v) => append_caption(
            &v.caption,
            &format!("{path}.voice_note.caption"),
            output,
            budget,
            depth + 1,
        )?,
        RichBlockKind::Divider(_) | RichBlockKind::Anchor(_) | RichBlockKind::Thinking(_) => {}
    }
    Ok(())
}

fn append_blocks(
    blocks: &[RichBlock],
    path: &str,
    output: &mut Vec<EmojiOccurrence>,
    budget: &mut TraversalBudget,
    depth: usize,
) -> Result<(), EmojiAnalysisError> {
    budget.visit(path, depth)?;
    for (index, block) in blocks.iter().enumerate() {
        append_block(
            block,
            &format!("{path}[{index}]"),
            output,
            budget,
            depth + 1,
        )?;
    }
    Ok(())
}

fn append_caption(
    caption: &Option<teloxide::types::RichBlockCaption>,
    path: &str,
    output: &mut Vec<EmojiOccurrence>,
    budget: &mut TraversalBudget,
    depth: usize,
) -> Result<(), EmojiAnalysisError> {
    budget.visit(path, depth)?;
    if let Some(caption) = caption {
        append_inline(
            &caption.text,
            &format!("{path}.text"),
            output,
            budget,
            depth + 1,
        )?;
        append_optional_inline(
            &caption.credit,
            &format!("{path}.credit"),
            output,
            budget,
            depth + 1,
        )?;
    }
    Ok(())
}

fn append_optional_inline(
    text: &Option<RichText>,
    path: &str,
    output: &mut Vec<EmojiOccurrence>,
    budget: &mut TraversalBudget,
    depth: usize,
) -> Result<(), EmojiAnalysisError> {
    if let Some(text) = text {
        append_inline(text, path, output, budget, depth + 1)?;
    }
    Ok(())
}

fn append_inline(
    text: &RichText,
    path: &str,
    output: &mut Vec<EmojiOccurrence>,
    budget: &mut TraversalBudget,
    depth: usize,
) -> Result<(), EmojiAnalysisError> {
    budget.visit(path, depth)?;
    match text {
        RichText::Text(text) => append_leaf(text, None, path, output, budget, depth + 1)?,
        RichText::List(items) => {
            for (index, item) in items.iter().enumerate() {
                append_inline(item, &format!("{path}[{index}]"), output, budget, depth + 1)?;
            }
        }
        RichText::Object(object) => match object {
            RichTextObject::Bold(v) => {
                append_inline(&v.text, &format!("{path}.bold"), output, budget, depth + 1)?
            }
            RichTextObject::Italic(v) => append_inline(
                &v.text,
                &format!("{path}.italic"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::Underline(v) => append_inline(
                &v.text,
                &format!("{path}.underline"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::Strikethrough(v) => append_inline(
                &v.text,
                &format!("{path}.strikethrough"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::Spoiler(v) => append_inline(
                &v.text,
                &format!("{path}.spoiler"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::DateTime(v) => append_inline(
                &v.text,
                &format!("{path}.datetime"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::TextMention(v) => append_inline(
                &v.text,
                &format!("{path}.text_mention"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::Subscript(v) => append_inline(
                &v.text,
                &format!("{path}.subscript"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::Superscript(v) => append_inline(
                &v.text,
                &format!("{path}.superscript"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::Marked(v) => append_inline(
                &v.text,
                &format!("{path}.marked"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::Code(v) => {
                append_inline(&v.text, &format!("{path}.code"), output, budget, depth + 1)?
            }
            RichTextObject::CustomEmoji(v) => append_leaf(
                &v.alternative_text,
                Some(v.custom_emoji_id.to_string()),
                &format!("{path}.custom_emoji"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::MathematicalExpression(v) => append_leaf(
                &v.expression,
                None,
                &format!("{path}.math"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::Url(v) => append_inline(
                &v.text,
                &format!("{path}.url.text"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::EmailAddress(v) => append_inline(
                &v.text,
                &format!("{path}.email.text"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::PhoneNumber(v) => append_inline(
                &v.text,
                &format!("{path}.phone.text"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::BankCardNumber(v) => append_inline(
                &v.text,
                &format!("{path}.bank_card.text"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::TonAddress(v) => append_inline(
                &v.text,
                &format!("{path}.ton_address.text"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::Mention(v) => append_inline(
                &v.text,
                &format!("{path}.mention.text"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::Hashtag(v) => append_inline(
                &v.text,
                &format!("{path}.hashtag.text"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::Cashtag(v) => append_inline(
                &v.text,
                &format!("{path}.cashtag.text"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::BotCommand(v) => append_inline(
                &v.text,
                &format!("{path}.bot_command.text"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::Button(v) => append_inline(
                &v.button.text,
                &format!("{path}.button.text"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::AnchorLink(v) => append_inline(
                &v.text,
                &format!("{path}.anchor_link.text"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::Reference(v) => append_inline(
                &v.text,
                &format!("{path}.reference.text"),
                output,
                budget,
                depth + 1,
            )?,
            RichTextObject::ReferenceLink(v) => append_inline(
                &v.text,
                &format!("{path}.reference_link.text"),
                output,
                budget,
                depth + 1,
            )?,
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
    budget: &mut TraversalBudget,
    depth: usize,
) -> Result<(), EmojiAnalysisError> {
    budget.visit(path, depth)?;
    budget.charge_text(text)?;
    let custom = custom_id
        .map(|id| CustomEmojiInput {
            id,
            utf16_range: 0..text.encode_utf16().count(),
            format: EmojiFormat::Unknown,
        })
        .into_iter()
        .collect::<Vec<_>>();
    let analysis = emoji_analysis::analyze(text, &custom)?;
    budget.charge_analysis(&analysis)?;
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
        let mut occurrences = Vec::new();
        let mut budget = TraversalBudget::default();
        budget.visit("quote.rich_texts", 0)?;
        for (index, text) in rich_texts.iter().enumerate() {
            append_inline(
                text,
                &format!("quote.rich_texts[{index}]"),
                &mut occurrences,
                &mut budget,
                1,
            )?;
        }
        references.extend(occurrences.into_iter().map(|item| EmojiReference {
            kind: ReferenceKind::Quote,
            identity: item.identity,
            fallback: Some(item.fallback),
        }));
    } else {
        let mut budget = TraversalBudget::default();
        budget.visit("quote.text", 0)?;
        budget.charge_text(&quote.text)?;
        let analysis = emoji_analysis::analyze_telegram_text(&quote.text, &quote.entities)?;
        budget.charge_analysis(&analysis)?;
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
    fn traversal_budget_enforces_exact_boundaries() {
        let mut depth = TraversalBudget::default();
        assert!(depth.visit("path", MAX_TRAVERSAL_DEPTH).is_ok());
        assert_eq!(
            depth.visit("path", MAX_TRAVERSAL_DEPTH + 1),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );

        let long_path = "p".repeat(MAX_SINGLE_PATH_BYTES);
        let mut aggregate_path = TraversalBudget::default();
        for _ in 0..(MAX_TRAVERSAL_PATH_BYTES / MAX_SINGLE_PATH_BYTES) {
            assert!(aggregate_path.visit(&long_path, 0).is_ok());
        }
        assert_eq!(
            aggregate_path.visit(&long_path, 0),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );

        let mut nodes = TraversalBudget {
            nodes: MAX_TRAVERSAL_NODES,
            ..TraversalBudget::default()
        };
        assert_eq!(
            nodes.visit("path", 0),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );

        let exact_path = "p".repeat(MAX_SINGLE_PATH_BYTES);
        let mut path = TraversalBudget::default();
        assert!(path.visit(&exact_path, 0).is_ok());
        assert_eq!(
            path.visit(&format!("{exact_path}x"), 0),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );

        let mut total_text = TraversalBudget::default();
        total_text.text_bytes = MAX_TOTAL_TEXT_BYTES - 1;
        assert!(total_text.charge_text("x").is_ok());
        assert_eq!(
            total_text.charge_text("x"),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );
    }

    #[test]
    fn typed_rich_traversal_stops_at_depth_nodes_text_and_output_budgets() {
        let wide = json!({
            "blocks": vec![json!({"type":"divider"}); MAX_TRAVERSAL_NODES]
        });
        assert_eq!(
            extract_rich_value(wide),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );

        let mut nested_block = json!({"type":"paragraph", "text":"😀"});
        for _ in 0..(MAX_TRAVERSAL_DEPTH + 2) {
            nested_block = json!({
                "type":"details",
                "summary":"",
                "blocks":[nested_block]
            });
        }
        assert_eq!(
            extract_rich_value(json!({"blocks":[nested_block]})),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );

        let too_much_text = json!({
            "blocks":[{"type":"paragraph", "text": "x".repeat(MAX_TOTAL_TEXT_BYTES + 1)}]
        });
        assert_eq!(
            extract_rich_value(too_much_text),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );

        let too_many_occurrences = json!({
            "blocks":[{"type":"paragraph", "text": vec!["😀😀😀😀"; 2_500]}]
        });
        assert_eq!(
            extract_rich_value(too_many_occurrences),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );

        let custom_emojis = (0..=MAX_TOTAL_CUSTOM_EMOJI)
            .map(|id| {
                json!({
                    "type":"custom_emoji",
                    "custom_emoji_id":id.to_string(),
                "alternative_text":"😀"
                })
            })
            .collect::<Vec<_>>();
        let too_many_custom_emoji = json!({
            "blocks":[{"type":"paragraph", "text":custom_emojis}]
        });
        assert_eq!(
            extract_rich_value(too_many_custom_emoji),
            Err(EmojiAnalysisError::RichTraversalLimitExceeded)
        );
    }

    fn extract_rich_value(
        value: serde_json::Value,
    ) -> Result<Vec<EmojiOccurrence>, EmojiAnalysisError> {
        let rich: RichMessage = serde_json::from_value(value).unwrap();
        let mut found = Vec::new();
        append_rich_message(&rich, &mut found, &mut TraversalBudget::default())?;
        Ok(found)
    }

    #[test]
    fn rich_nested_paths_keep_custom_and_unicode_occurrences_separate() {
        let rich: RichMessage = serde_json::from_value(json!({"blocks":[
            {"type":"paragraph", "text":{"type":"bold", "text":["😀", {"type":"custom_emoji", "custom_emoji_id":"99", "alternative_text":"😎"}]}}
        ]})).unwrap();
        let mut found = Vec::new();
        append_rich_message(&rich, &mut found, &mut TraversalBudget::default()).unwrap();
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
