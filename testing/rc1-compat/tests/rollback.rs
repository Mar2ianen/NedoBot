use serde_json::{Value, json};
use teloxide_core::types::Message as Rc1Message;
use tg_ai_bot_teloxide::telegram::entities::{
    deserialize_stored_message, message_url_targets, serialize_stored_message,
};

fn envelope(blocks: Value) -> Value {
    json!({
        "message_id": 1,
        "date": 1700000000,
        "chat": {"id": -10042, "type": "supergroup", "title": "Synthetic"},
        "rich_message": {"blocks": blocks}
    })
}

#[test]
fn actual_rc1_decodes_rc2_storage_and_rc2_reads_rc1_rewrite() {
    let opaque = json!({"type": "future_block", "payload": {
        "type": "expandable_blockquote", "data": "opaque"
    }});
    let rc2 = deserialize_stored_message(envelope(json!([
        {"type": "expandable_blockquote", "blocks": [
            {"type": "paragraph", "text": {"type": "url", "text": "link",
                "url": "https://example.org/link"}},
            {"type": "details", "summary": "Title", "blocks": [
                {"type": "buttons", "buttons": [
                    {"text": "button", "url": "https://example.org/button"}
                ]}
            ]}
        ], "credit": "source"},
        opaque
    ])))
    .unwrap();
    let stored = serialize_stored_message(&rc2).unwrap();
    let rc1: Rc1Message = serde_json::from_value(stored.clone()).unwrap();
    let rewritten = serde_json::to_value(&rc1).unwrap();
    let quote = &rewritten["rich_message"]["blocks"][0];
    assert!(quote.get("blocks").is_none());
    assert_eq!(quote["credit"], "source");
    let text = quote["text"].as_array().unwrap();
    assert!(text.iter().any(|v| v["url"] == "https://example.org/link"));
    assert!(text.iter().any(|v| v["button"]["url"] == "https://example.org/button"));
    assert_eq!(rewritten["rich_message"]["blocks"][1], opaque);
    let restored = deserialize_stored_message(rewritten).unwrap();
    let mut urls: Vec<_> = message_url_targets(&restored)
        .into_iter()
        .map(|target| target.value)
        .collect();
    urls.sort();
    urls.dedup();
    assert_eq!(urls, ["https://example.org/button", "https://example.org/link"]);
    assert_eq!(
        serde_json::to_value(deserialize_stored_message(stored).unwrap()).unwrap(),
        serde_json::to_value(rc2).unwrap()
    );
}
