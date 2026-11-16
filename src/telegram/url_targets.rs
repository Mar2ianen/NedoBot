//! Raw Telegram URL extraction adapter. Parsing semantics are owned by `teloxide-url`.

use teloxide::types::Message;
use teloxide_url::{Found, Kind};

pub(crate) fn extract_message_evidence(message: &Message) -> Vec<Found> {
    teloxide_url::adapter::extract_from_message(message)
}

pub(crate) fn extract_message_targets(message: &Message) -> Vec<Found> {
    extract_message_evidence(message)
        .into_iter()
        .filter(|target| matches!(target.kind, Kind::Url))
        .collect()
}
