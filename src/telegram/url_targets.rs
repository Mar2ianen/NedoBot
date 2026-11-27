//! Raw Telegram URL extraction adapter. Parsing semantics are owned by `teloxide-url`.

use teloxide::types::Message;
use teloxide_url::{Found, Kind};

pub(crate) fn extract_message_evidence(message: &Message) -> Vec<Found> {
    teloxide_url::adapter::extract_from_message(message)
}

pub(crate) fn extract_message_targets(message: &Message) -> Vec<Found> {
    let targets: Vec<_> = extract_message_evidence(message)
        .into_iter()
        .filter(|target| matches!(target.kind, Kind::Url))
        .collect();

    // Only the opt-in emoji extractor can validate a custom URL fallback's
    // addressed label and span. Keep all raw evidence elsewhere for audit.
    #[cfg(feature = "emoji")]
    let targets = {
        use std::{collections::HashSet, ops::Range};

        use crate::features::emoji_analysis::{EmojiEncoding, EmojiIdentity};

        let Ok(occurrences) = crate::features::emoji_occurrences::extract_message(message) else {
            return targets;
        };
        let content_source = if message.text().is_some() {
            crate::features::emoji_occurrences::ContentSource::Text
        } else {
            crate::features::emoji_occurrences::ContentSource::Caption
        };
        let mut validated = HashSet::<(u64, Range<usize>, String)>::new();
        for occurrence in occurrences {
            if occurrence.encoding != EmojiEncoding::CustomUrlFallback {
                continue;
            }
            if occurrence.source != content_source {
                // RichMessage URL findings do not yet carry a typed node path,
                // so matching by label alone could suppress a different link.
                continue;
            }
            let EmojiIdentity::Custom(id) = occurrence.identity else {
                continue;
            };
            let Ok(id) = id.parse::<u64>() else {
                continue;
            };
            validated.insert((id, occurrence.utf8_range, occurrence.fallback));
        }

        let mut filtered = targets;
        filtered.retain(|target| {
            let (Some(id), Some(span)) = (target.custom_emoji_fallback_id, target.span.as_ref())
            else {
                return true;
            };
            !validated.contains(&(id, span.clone(), target.label.clone().unwrap_or_default()))
        });
        filtered
    };

    targets
}
