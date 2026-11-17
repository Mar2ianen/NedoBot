use super::rich_content::{project, rich_message};
use std::borrow::Cow;
use teloxide::types::{Message, MessageEntityKind, MessageId, MessageOrigin};

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

pub fn custom_emoji_ids(msg: &Message) -> Vec<String> {
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
    teloxide_url::adapter::extract_from_message(msg)
        .into_iter()
        .filter(|target| matches!(target.kind, teloxide_url::Kind::Url))
        .collect()
}

/// Диапазон Telegram entity: offsets/length заданы в UTF-16, не UTF-8 байтах.
#[cfg(any(feature = "ask", feature = "moderation"))]
pub(crate) fn utf16_range(
    text: &str,
    offset: usize,
    length: usize,
) -> Option<std::ops::Range<usize>> {
    let end = offset.checked_add(length)?;
    fn boundary(text: &str, target: usize) -> Option<usize> {
        let mut units = 0usize;
        for (byte, character) in text.char_indices() {
            if units == target {
                return Some(byte);
            }
            units = units.checked_add(character.len_utf16())?;
            if units > target {
                return None;
            }
        }
        (units == target).then_some(text.len())
    }
    Some(boundary(text, offset)?..boundary(text, end)?)
}

#[cfg(test)]
mod tests {
    #[cfg(any(feature = "ask", feature = "moderation"))]
    use super::utf16_range;
    use super::{message_has_links, message_url_targets};
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
}
