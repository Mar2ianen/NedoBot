#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::Value;
use tg_ai_bot_teloxide::telegram::entities::{
    deserialize_stored_message, message_text, message_url_targets, serialize_stored_message,
};

fuzz_target!(|input: &[u8]| {
    if input.len() > 64 * 1024 {
        return;
    }
    let Ok(value) = serde_json::from_slice::<Value>(input) else {
        return;
    };
    // Accept full Message JSON as well as standalone blocks wrapped in a
    // minimal synthetic Message, to spend more cycles in the rich decoder.
    let value = if value.is_array() {
        serde_json::json!({
            "message_id": 1,
            "date": 1700000000,
            "chat": {"id": -10042, "type": "supergroup", "title": "Synthetic"},
            "rich_message": {"blocks": value}
        })
    } else {
        value
    };
    let Ok(message) = deserialize_stored_message(value) else {
        return;
    };
    let stored = serialize_stored_message(&message).expect("typed message must be storable");
    let restored = deserialize_stored_message(stored).expect("stored message must be readable");
    assert_eq!(
        serde_json::to_value(&message).unwrap(),
        serde_json::to_value(&restored).unwrap()
    );
    let _ = message_text(&restored);
    let _ = message_url_targets(&restored);
});
