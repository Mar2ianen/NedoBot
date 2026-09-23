use teloxide::types::{Message, MessageId};

use crate::config::Config;
use crate::telegram::entities::{forwarded_channel_post, message_text};

pub struct CommentCandidate<'a> {
    pub source_channel_id: i64,
    pub source_message_id: MessageId,
    pub post_text: &'a str,
    pub post_signature_marker: String,
}

pub fn comment_candidate<'a>(msg: &'a Message, config: &Config) -> Option<CommentCandidate<'a>> {
    if !msg.is_automatic_forward() {
        return None;
    }
    let (source_channel_id, source_message_id) = forwarded_channel_post(msg)?;
    let post_text = message_text(msg)?;
    let route = config
        .first_comment_route_for_chat(msg.chat.id.0)
        .find(|route| route.source_channel_id == source_channel_id)?;
    Some(CommentCandidate {
        source_channel_id,
        source_message_id,
        post_text,
        post_signature_marker: route.post_signature_marker.clone(),
    })
}
