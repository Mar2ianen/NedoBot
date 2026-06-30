use teloxide::types::{Message, MessageId};

use crate::config::Config;
use crate::telegram::entities::{forwarded_channel_post, message_text};

pub struct CommentCandidate<'a> {
    pub source_channel_id: i64,
    pub source_message_id: MessageId,
    pub post_text: &'a str,
}

pub fn comment_candidate<'a>(msg: &'a Message, config: &Config) -> Option<CommentCandidate<'a>> {
    match (
        msg.chat.id.0 == config.discussion_chat_id,
        msg.is_automatic_forward(),
        forwarded_channel_post(msg),
        message_text(msg),
    ) {
        (true, true, Some((source_channel_id, source_message_id)), Some(post_text))
            if source_channel_id == config.source_channel_id =>
        {
            Some(CommentCandidate {
                source_channel_id,
                source_message_id,
                post_text,
            })
        }
        _ => None,
    }
}
