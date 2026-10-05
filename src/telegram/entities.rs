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
    let text_has_links = message_text(msg)
        .map(|text| text.contains("http://") || text.contains("https://") || text.contains("t.me/"))
        .unwrap_or(false);

    text_has_links
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
