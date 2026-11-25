//! Технические identities не доказывают личность человека за каналом.
/// Telegram использует этот proxy sender для сообщений от имени канала.
pub const CHANNEL_SENDER_PROXY_ID: i64 = 136_817_688;

/// Проверяем стабильный ID, а не username, который может скопировать другой автор.
pub fn is_channel_sender_proxy(user_id: i64) -> bool {
    user_id == CHANNEL_SENDER_PROXY_ID
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proxy_identity_does_not_include_neighboring_human_ids() {
        assert!(is_channel_sender_proxy(CHANNEL_SENDER_PROXY_ID));
        assert!(!is_channel_sender_proxy(CHANNEL_SENDER_PROXY_ID - 1));
        assert!(!is_channel_sender_proxy(CHANNEL_SENDER_PROXY_ID + 1));
    }
}
