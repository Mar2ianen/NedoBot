use super::rich_content::{legacy_quote_text, project, rich_message};
use std::borrow::Cow;
use teloxide::types::{Message, MessageEntityKind, MessageId, MessageOrigin, RichBlock};

/// Persist both Bot API 10.4 and 10.5 expandable-quote fields. RC1 readers
/// require `text` and ignore the newer `blocks`; RC2 readers use `blocks`.
pub fn serialize_stored_message(message: &Message) -> anyhow::Result<serde_json::Value> {
    let mut value = serde_json::to_value(message)?;
    add_legacy_expandable_quote_text(&mut value)?;
    Ok(value)
}

pub fn serialize_stored_messages(messages: &[Message]) -> anyhow::Result<serde_json::Value> {
    messages
        .iter()
        .map(serialize_stored_message)
        .collect::<anyhow::Result<Vec<_>>>()
        .map(serde_json::Value::Array)
}

/// Upgrade stored Bot API 10.4 expandable quotes before deserializing with RC2.
/// Keeping `text` in dual-format rows also allows rolling back to the RC1 binary.
pub fn deserialize_stored_message(mut value: serde_json::Value) -> serde_json::Result<Message> {
    add_expandable_quote_blocks(&mut value);
    serde_json::from_value(value)
}

// Only visit typed Message and RichMessage containers. Unknown rich blocks and
// inline objects are opaque, including any fields named "type" or "blocks".
fn visit_stored_quotes<E>(
    root: &mut serde_json::Value,
    mut visit: impl FnMut(&mut serde_json::Map<String, serde_json::Value>) -> Result<(), E>,
) -> Result<(), E> {
    use serde_json::Value;

    enum Node {
        Message,
        RichMessage,
        Blocks,
        ListItems,
        NestedMessage(&'static str),
    }

    let mut pending = vec![(root, Node::Message)];
    while let Some((value, node)) = pending.pop() {
        match node {
            Node::Message => {
                if let Some(object) = value.as_object_mut() {
                    for (key, child) in object {
                        let node = match key.as_str() {
                            "rich_message" => Node::RichMessage,
                            "reply_to_message" | "pinned_message" => Node::Message,
                            "giveaway_completed" => Node::NestedMessage("giveaway_message"),
                            "suggested_post_approved"
                            | "suggested_post_approval_failed"
                            | "suggested_post_declined"
                            | "suggested_post_paid"
                            | "suggested_post_refunded" => {
                                Node::NestedMessage("suggested_post_message")
                            }
                            _ => continue,
                        };
                        pending.push((child, node));
                    }
                }
            }
            Node::NestedMessage(field) => {
                if let Some(message) = value.get_mut(field) {
                    pending.push((message, Node::Message));
                }
            }
            Node::RichMessage => {
                if let Some(blocks) = value.get_mut("blocks") {
                    pending.push((blocks, Node::Blocks));
                }
            }
            Node::Blocks => {
                let Some(blocks) = value.as_array_mut() else {
                    continue;
                };
                for block in blocks {
                    let Some(object) = block.as_object_mut() else {
                        continue;
                    };
                    let node = match object.get("type").and_then(Value::as_str) {
                        Some("expandable_blockquote") => {
                            visit(object)?;
                            Some(("blocks", Node::Blocks))
                        }
                        Some("blockquote" | "collage" | "slideshow" | "details") => {
                            Some(("blocks", Node::Blocks))
                        }
                        Some("list") => Some(("items", Node::ListItems)),
                        _ => None,
                    };
                    if let Some((field, node)) = node
                        && let Some(child) = object.get_mut(field)
                    {
                        pending.push((child, node));
                    }
                }
            }
            Node::ListItems => {
                if let Some(items) = value.as_array_mut() {
                    for item in items {
                        if let Some(blocks) = item.get_mut("blocks") {
                            pending.push((blocks, Node::Blocks));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn add_legacy_expandable_quote_text(value: &mut serde_json::Value) -> anyhow::Result<()> {
    visit_stored_quotes(value, |object| {
        if !object.contains_key("text") {
            let blocks = object
                .get("blocks")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("expandable quote has no blocks"))?;
            let blocks: Vec<RichBlock> = serde_json::from_value(blocks)?;
            object.insert(
                "text".to_owned(),
                serde_json::to_value(legacy_quote_text(&blocks))?,
            );
        }
        Ok(())
    })
}

fn add_expandable_quote_blocks(value: &mut serde_json::Value) {
    let result: Result<(), std::convert::Infallible> = visit_stored_quotes(value, |object| {
        if !object.contains_key("blocks")
            && let Some(text) = object.get("text").cloned()
        {
            object.insert(
                "blocks".to_owned(),
                serde_json::json!([{"type": "paragraph", "text": text}]),
            );
        }
        Ok(())
    });
    match result {
        Ok(()) => {}
        Err(never) => match never {},
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelegramMessageRef {
    pub chat_id: i64,
    pub message_id: i32,
}

impl TelegramMessageRef {
    pub fn from_message(message: &Message) -> Self {
        Self {
            chat_id: message.chat.id.0,
            message_id: message.id.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRevision {
    Original,
    Edited { edit_date_unix: Option<i64> },
}

impl MessageRevision {
    pub fn from_message(message: &Message) -> Self {
        match message.edit_date() {
            Some(edit_date) => Self::Edited {
                edit_date_unix: Some(edit_date.timestamp()),
            },
            None => Self::Original,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TelegramMessageProjection {
    pub message: TelegramMessageRef,
    pub source_channel_id: Option<i64>,
    pub source_message_id: Option<i32>,
    pub user_id: Option<i64>,
    pub reply_to_message_id: Option<i32>,
    pub reply_to_user_id: Option<i64>,
    pub sender_chat_id: Option<i64>,
    pub via_bot_id: Option<i64>,
    pub text: Option<String>,
    pub raw_json: serde_json::Value,
    pub has_photo: bool,
    pub has_video: bool,
    pub has_document: bool,
    pub has_audio: bool,
    pub has_voice: bool,
    pub has_sticker: bool,
    pub has_animation: bool,
    pub has_links: bool,
    pub is_forwarded: bool,
    pub is_automatic_forward: bool,
}

impl TelegramMessageProjection {
    pub fn from_message(message: &Message) -> anyhow::Result<Self> {
        let (source_channel_id, source_message_id) = forwarded_channel_post(message)
            .map(|(chat_id, message_id)| (Some(chat_id), Some(message_id.0)))
            .unwrap_or((None, None));
        let reply = message.reply_to_message();

        Ok(Self {
            message: TelegramMessageRef::from_message(message),
            source_channel_id,
            source_message_id,
            user_id: message.from.as_ref().map(|user| user.id.0 as i64),
            reply_to_message_id: reply.map(|reply| reply.id.0),
            reply_to_user_id: reply
                .and_then(|reply| reply.from.as_ref())
                .map(|user| user.id.0 as i64),
            sender_chat_id: message.sender_chat.as_ref().map(|chat| chat.id.0),
            via_bot_id: message.via_bot.as_ref().map(|bot| bot.id.0 as i64),
            text: message_text(message).map(Cow::into_owned),
            raw_json: serialize_stored_message(message)?,
            has_photo: message.photo().is_some(),
            has_video: message.video().is_some(),
            has_document: message.document().is_some(),
            has_audio: message.audio().is_some(),
            has_voice: message.voice().is_some(),
            has_sticker: message.sticker().is_some(),
            has_animation: message.animation().is_some(),
            has_links: message_has_links(message),
            is_forwarded: message.forward_origin().is_some(),
            is_automatic_forward: message.is_automatic_forward(),
        })
    }
}

pub fn forwarded_channel_post(msg: &Message) -> Option<(i64, MessageId)> {
    match msg.forward_origin()? {
        MessageOrigin::Channel {
            chat, message_id, ..
        } => Some((chat.id.0, *message_id)),
        _ => None,
    }
}

pub fn message_text(msg: &Message) -> Option<Cow<'_, str>> {
    msg.text()
        .or_else(|| msg.caption())
        .map(Cow::Borrowed)
        .or_else(|| {
            let text = project(rich_message(msg)?).text;
            (!text.is_empty()).then_some(Cow::Owned(text))
        })
}

#[cfg(feature = "emoji")]
pub type CustomEmojiIdsError = crate::features::emoji_analysis::EmojiAnalysisError;
#[cfg(not(feature = "emoji"))]
pub type CustomEmojiIdsError = std::convert::Infallible;

pub fn custom_emoji_ids(msg: &Message) -> Result<Vec<String>, CustomEmojiIdsError> {
    #[cfg(feature = "emoji")]
    {
        let occurrences = crate::features::emoji_occurrences::extract_message(msg)?;
        Ok(occurrences
            .into_iter()
            .filter_map(|occurrence| match occurrence.kind {
                crate::features::emoji_analysis::EmojiKind::Custom { id, .. } => Some(id.0),
                crate::features::emoji_analysis::EmojiKind::Unicode { .. } => None,
            })
            .collect())
    }

    #[cfg(not(feature = "emoji"))]
    Ok(legacy_custom_emoji_ids(msg))
}

#[cfg(not(feature = "emoji"))]
fn legacy_custom_emoji_ids(msg: &Message) -> Vec<String> {
    let mut ids: Vec<_> = msg
        .entities()
        .into_iter()
        .flatten()
        .chain(msg.caption_entities().into_iter().flatten())
        .filter_map(|entity| match &entity.kind {
            MessageEntityKind::CustomEmoji { custom_emoji_id } => Some(custom_emoji_id.to_string()),
            _ => None,
        })
        .collect();
    if let Some(rich) = rich_message(msg) {
        ids.extend(project(rich).custom_emoji_ids);
    }
    ids
}

pub fn message_has_links(msg: &Message) -> bool {
    // Сохраняем прежнюю семантику счётчика на уровне сообщения. Полные цели
    // ссылок доступны отдельно через `message_url_targets`.
    message_text(msg)
        .map(|text| text.contains("http://") || text.contains("https://") || text.contains("t.me/"))
        .unwrap_or(false)
        || rich_message(msg).is_some_and(|rich| project(rich).has_links)
        || msg
            .entities()
            .into_iter()
            .flatten()
            .chain(msg.caption_entities().into_iter().flatten())
            .any(|entity| {
                matches!(
                    entity.kind,
                    MessageEntityKind::Url | MessageEntityKind::TextLink { .. }
                )
            })
}

/// Извлекает типизированные цели URL. Упоминания и хештеги не считаются URL.
pub fn message_url_targets(msg: &Message) -> Vec<teloxide_url::Found> {
    super::url_targets::extract_message_targets(msg)
}

/// Диапазон Telegram entity: offsets/length заданы в UTF-16, не UTF-8 байтах.
#[cfg(any(feature = "ask", feature = "moderation"))]
pub(crate) fn utf16_range(
    text: &str,
    offset: usize,
    length: usize,
) -> Option<std::ops::Range<usize>> {
    teloxide::types::Utf16OffsetMap::new(text).range(offset, length)
}

#[cfg(test)]
mod tests {
    #[cfg(any(feature = "ask", feature = "moderation"))]
    use super::utf16_range;
    use super::{
        MessageRevision, TelegramMessageProjection, deserialize_stored_message, message_has_links,
        message_text, message_url_targets, serialize_stored_messages,
    };
    use teloxide::types::Message;

    #[test]
    fn hidden_text_link_is_a_link_even_when_the_label_is_not() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 7,
            "date": 1_700_000_000,
            "chat": {"id": 42, "type": "private"},
            "from": {"id": 13, "is_bot": false, "first_name": "Test"},
            "text": "открыть",
            "entities": [{
                "type": "text_link", "offset": 0, "length": 7,
                "url": "https://example.org/hidden"
            }]
        }))
        .unwrap();

        assert!(message_has_links(&message));
        let targets = message_url_targets(&message);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].value, "https://example.org/hidden");
        assert_eq!(targets[0].label.as_deref(), Some("открыть"));

        let projection = TelegramMessageProjection::from_message(&message).unwrap();
        assert_eq!(projection.message.chat_id, 42);
        assert_eq!(projection.message.message_id, 7);
        assert_eq!(projection.user_id, Some(13));
        assert_eq!(projection.text.as_deref(), Some("открыть"));
        assert!(projection.has_links);
        assert!(!projection.is_forwarded);
        assert_eq!(
            MessageRevision::from_message(&message),
            MessageRevision::Original
        );
    }

    #[cfg(feature = "emoji")]
    #[test]
    fn emoji_ids_include_only_valid_custom_url_fallbacks_when_emoji_feature_is_enabled() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 7,
            "date": 1_700_000_000,
            "chat": {"id": 42, "type": "private"},
            "text": "😀 open",
            "entities": [
                {"type":"text_link", "offset":0, "length":2, "url":"tg://emoji?id=00042"},
                {"type":"text_link", "offset":3, "length":4, "url":"tg://emoji?id=99"}
            ]
        }))
        .unwrap();
        assert_eq!(super::custom_emoji_ids(&message).unwrap(), ["42"]);
    }

    #[cfg(feature = "emoji")]
    #[test]
    fn strict_emoji_id_extraction_does_not_downgrade_invalid_ranges_to_legacy_entities() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 7,
            "date": 1_700_000_000,
            "chat": {"id": 42, "type": "private"},
            "text": "😀",
            "entities": [{"type":"custom_emoji", "offset":1, "length":1, "custom_emoji_id":"legacy"}]
        })).unwrap();
        assert!(super::custom_emoji_ids(&message).is_err());
    }

    #[cfg(feature = "emoji")]
    #[test]
    fn only_valid_custom_fallback_targets_are_removed_from_url_targets() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 8,
            "date": 1_700_000_000,
            "chat": {"id": 42, "type": "private"},
            "text": "😀 open https://example.org",
            "entities": [
                {"type":"text_link", "offset":0, "length":2, "url":"tg://emoji?id=42"},
                {"type":"text_link", "offset":3, "length":4, "url":"tg://emoji?id=99"},
                {"type":"url", "offset":8, "length":19}
            ]
        }))
        .unwrap();
        let targets = message_url_targets(&message);
        assert_eq!(targets.len(), 2);
        assert!(
            targets
                .iter()
                .any(|target| target.value == "tg://emoji?id=99")
        );
        assert!(
            targets
                .iter()
                .any(|target| target.value == "https://example.org")
        );
        assert!(
            !targets
                .iter()
                .any(|target| target.value == "tg://emoji?id=42")
        );
    }

    #[cfg(feature = "emoji")]
    #[test]
    fn native_custom_entity_with_same_id_and_fallback_is_filtered_by_provenance() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 9,
            "date": 1_700_000_000,
            "chat": {"id": 42, "type": "private"},
            "text": "😀",
            "entities": [
                {"type":"custom_emoji", "offset":0, "length":2, "custom_emoji_id":"42"},
                {"type":"text_link", "offset":0, "length":2, "url":"tg://emoji?id=42"}
            ]
        }))
        .unwrap();
        assert!(message_url_targets(&message).is_empty());
    }

    #[test]
    fn anonymous_message_projection_keeps_sender_chat_without_fabricating_user_id() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 9,
            "date": 1_700_000_000,
            "chat": {"id": -10042, "type": "supergroup"},
            "sender_chat": {"id": -10043, "type": "channel", "title": "Synthetic sender"},
            "text": "synthetic anonymous post"
        }))
        .unwrap();

        let projection = TelegramMessageProjection::from_message(&message).unwrap();
        assert_eq!(projection.message.chat_id, -10042);
        assert_eq!(projection.message.message_id, 9);
        assert_eq!(projection.user_id, None);
        assert_eq!(projection.sender_chat_id, Some(-10043));
        assert!(!projection.is_forwarded);
    }

    #[test]
    fn mention_and_hashtag_do_not_change_historical_link_flag() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 8,
            "date": 1_700_000_000,
            "chat": {"id": 42, "type": "private"},
            "from": {"id": 13, "is_bot": false, "first_name": "Test"},
            "text": "@alice #news",
            "entities": [
                {"type": "mention", "offset": 0, "length": 6},
                {"type": "hashtag", "offset": 7, "length": 5}
            ]
        }))
        .unwrap();

        assert!(!message_has_links(&message));
    }

    #[cfg(any(feature = "ask", feature = "moderation"))]
    #[test]
    fn utf16_entity_ranges_accept_codepoint_boundaries_and_reject_surrogate_splits() {
        let text = "😀x";
        assert_eq!(utf16_range(text, 0, 2), Some(0..4));
        assert_eq!(utf16_range(text, 2, 1), Some(4..5));
        assert_eq!(utf16_range(text, 1, 1), None);
        assert_eq!(utf16_range(text, 3, 1), None);
        assert_eq!(utf16_range(text, usize::MAX, 1), None);
    }

    #[test]
    fn caption_entities_use_utf16_spans_and_keep_the_hidden_link_target() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 9,
            "date": 1_700_000_000,
            "chat": {"id": 42, "type": "private"},
            "from": {"id": 13, "is_bot": false, "first_name": "Test"},
            "photo": [{"file_id": "synthetic", "file_unique_id": "synthetic", "width": 1, "height": 1}],
            "caption": "😺open",
            "caption_entities": [{"type": "text_link", "offset": 2, "length": 4,
                "url": "https://hidden.example/caption?synthetic=1"}]
        })).unwrap();

        let targets = message_url_targets(&message);
        assert_eq!(targets.len(), 1);
        assert_eq!(
            targets[0].value,
            "https://hidden.example/caption?synthetic=1"
        );
        assert_eq!(targets[0].label.as_deref(), Some("open"));
        assert_eq!(targets[0].span, Some(4..8));
    }

    #[test]
    fn stored_rich_quotes_round_trip_both_rc1_and_rc2_wire_fields() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 10,
            "date": 1_700_000_000,
            "chat": {"id": -10042, "type": "supergroup"},
            "rich_message": {"blocks": [{
                "type": "expandable_blockquote",
                "blocks": [
                    {"type": "paragraph", "text": "Цитата"},
                    {"type": "paragraph", "text": {"type": "url", "url": "https://example.org/hidden", "text": "ссылка"}}
                ],
                "credit": "источник"
            }]}
        }))
        .unwrap();

        let projection = TelegramMessageProjection::from_message(&message).unwrap();
        let stored = projection.raw_json;
        assert_eq!(
            serialize_stored_messages(std::slice::from_ref(&message)).unwrap()[0],
            stored
        );
        let quote = stored.pointer("/rich_message/blocks/0").unwrap();
        assert_eq!(quote["text"][0], "Цитата");
        assert_eq!(quote["text"][1], "\n");
        assert_eq!(
            quote["text"][2],
            serde_json::json!({
                "type": "url",
                "url": "https://example.org/hidden",
                "text": "ссылка"
            })
        );
        assert_eq!(quote["blocks"][0]["text"], "Цитата");
        assert_eq!(quote["credit"], "источник");
        assert!(
            quote.get("blocks").is_some(),
            "RC2 readers require the blocks field"
        );
        assert!(
            quote.get("text").is_some(),
            "RC1 rollback readers require the legacy text field"
        );

        let restored = deserialize_stored_message(stored).unwrap();
        assert_eq!(
            message_text(&restored).as_deref(),
            Some("Цитата\nссылка\n\nисточник")
        );
        assert_eq!(
            message_url_targets(&restored)
                .iter()
                .map(|found| found.value.as_str())
                .collect::<Vec<_>>(),
            ["https://example.org/hidden"]
        );
    }

    #[test]
    fn generated_stored_quotes_preserve_typed_content_and_legacy_url_targets() {
        use proptest::{
            prelude::*,
            test_runner::{Config, RngSeed, TestRunner},
        };
        use serde_json::json;
        use std::collections::BTreeSet;

        let seed = match std::env::var("NEDOBOT_PROPTEST_SEED") {
            Ok(value) => value.parse::<u64>().expect("invalid property seed"),
            Err(std::env::VarError::NotPresent) => 0x15_01_02_03_04_05_06_07,
            Err(error) => panic!("invalid property seed: {error}"),
        };
        let strategy = prop::collection::vec(
            (
                prop_oneof![Just("🦀 Ж".to_owned()), "[a-z]{1,12}"],
                any::<bool>(),
                prop::collection::vec(0_u8..4, 0..=4),
            ),
            1..=6,
        );
        let mut runner = TestRunner::new(Config {
            cases: 256,
            rng_seed: RngSeed::Fixed(seed),
            failure_persistence: None,
            ..Config::default()
        });
        runner
            .run(&strategy, |leaves| {
                let mut expected = BTreeSet::new();
                let mut blocks = Vec::new();
                for (index, (label, button, wrappers)) in leaves.iter().enumerate() {
                    let url = format!("https://example.org/generated/{index}");
                    expected.insert(url.clone());
                    let mut block = if *button {
                        json!({"type": "buttons", "buttons": [{"text": label, "url": url}]})
                    } else {
                        json!({"type": "paragraph", "text": {
                            "type": "url", "text": label, "url": url
                        }})
                    };
                    for wrapper in wrappers {
                        block = match wrapper {
                            0 => json!({"type": "blockquote", "blocks": [block]}),
                            1 => json!({"type": "expandable_blockquote", "blocks": [block]}),
                            2 => json!({"type": "details", "summary": "Details", "blocks": [block]}),
                            _ => json!({"type": "list", "items": [{"label": "1.", "blocks": [block]}]}),
                        };
                    }
                    blocks.push(block);
                }
                let opaque = json!({"type": "future_block", "payload": {
                    "type": "expandable_blockquote", "data": "opaque"
                }});
                blocks.push(opaque.clone());
                let raw = json!({
                    "message_id": 14,
                    "date": 1_700_000_000,
                    "chat": {"id": -10042, "type": "supergroup"},
                    "rich_message": {"blocks": [{
                        "type": "expandable_blockquote", "blocks": blocks
                    }]}
                });
                let message: Message = serde_json::from_value(raw).unwrap();
                let canonical = serde_json::to_value(&message).unwrap();
                let stored = super::serialize_stored_message(&message).unwrap();
                let quote = &stored["rich_message"]["blocks"][0];
                prop_assert_eq!(&quote["blocks"][leaves.len()], &opaque);
                prop_assert!(canonical["rich_message"]["blocks"][0].get("text").is_none());
                let restored = deserialize_stored_message(stored.clone()).unwrap();
                prop_assert_eq!(serde_json::to_value(&restored).unwrap(), canonical);

                let urls = |message: &Message| {
                    message_url_targets(message)
                        .into_iter()
                        .map(|target| target.value)
                        .collect::<BTreeSet<_>>()
                };
                prop_assert_eq!(urls(&restored), expected.clone());
                // Exercise the legacy-only field independently from the RC2
                // blocks. An actual RC1 binary is a separate compatibility gate.
                let legacy = json!({
                    "message_id": 14,
                    "date": 1_700_000_000,
                    "chat": {"id": -10042, "type": "supergroup"},
                    "rich_message": {"blocks": [{
                        "type": "expandable_blockquote", "text": quote["text"]
                    }]}
                });
                let restored_legacy = deserialize_stored_message(legacy).unwrap();
                prop_assert_eq!(urls(&restored_legacy), expected);

                let mut repeated = stored.clone();
                super::add_legacy_expandable_quote_text(&mut repeated).unwrap();
                prop_assert_eq!(repeated, stored);
                Ok(())
            })
            .unwrap_or_else(|error| {
                panic!(
                    "case_id=stored-rich-quotes seed={seed} sha={} replay=sh scripts/replay_quality_case.sh stored-rich-quotes {seed}: {error}",
                    std::env::var("NEDOBOT_TEST_SHA")
                        .unwrap_or_else(|_| "local-unpinned".to_owned())
                )
            });
    }

    #[test]
    fn stored_quote_normalization_preserves_opaque_blocks_and_nested_replies() {
        let opaque = serde_json::json!({
            "type": "future_block",
            "payload": {
                "type": "expandable_blockquote",
                "text": "opaque",
                "nested": {"type": "expandable_blockquote", "data": "opaque"}
            }
        });
        let mut raw = serde_json::json!({
            "message_id": 12,
            "date": 1_700_000_000,
            "chat": {"id": -10042, "type": "supergroup"},
            "rich_message": {"blocks": [opaque.clone()]},
            "reply_to_message": {
                "message_id": 11,
                "date": 1_700_000_000,
                "chat": {"id": -10042, "type": "supergroup"},
                "rich_message": {"blocks": [{
                    "type": "expandable_blockquote",
                    "text": "Legacy"
                }]}
            }
        });
        super::add_expandable_quote_blocks(&mut raw);
        let once = raw.clone();
        super::add_expandable_quote_blocks(&mut raw);
        assert_eq!(raw, once);
        assert_eq!(raw["rich_message"]["blocks"][0], opaque);
        let message = deserialize_stored_message(raw).unwrap();
        let mut stored = super::serialize_stored_message(&message).unwrap();
        assert_eq!(stored["rich_message"]["blocks"][0], opaque);
        assert_eq!(
            stored["reply_to_message"]["rich_message"]["blocks"][0]["text"],
            serde_json::json!(["Legacy"])
        );
        let once = stored.clone();
        super::add_legacy_expandable_quote_text(&mut stored).unwrap();
        assert_eq!(stored, once);
    }

    #[test]
    fn stored_rc1_expandable_quote_is_upgraded_for_rc2_deserialization() {
        let old_payload = serde_json::json!({
            "message_id": 11,
            "date": 1_700_000_000,
            "chat": {"id": -10042, "type": "supergroup"},
            "rich_message": {"blocks": [{
                "type": "expandable_blockquote",
                "text": [
                    {"type": "bold", "text": "Старая цитата"},
                    " ",
                    {"type": "url", "url": "https://example.org/legacy", "text": "ссылка"}
                ],
                "credit": "старый источник"
            }]}
        });

        let restored = deserialize_stored_message(old_payload).unwrap();
        assert_eq!(
            message_text(&restored).as_deref(),
            Some("Старая цитата ссылка\n\nстарый источник")
        );
        assert_eq!(
            message_url_targets(&restored)
                .iter()
                .map(|found| found.value.as_str())
                .collect::<Vec<_>>(),
            ["https://example.org/legacy"]
        );
    }
}
