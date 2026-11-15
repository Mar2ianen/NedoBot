use teloxide::types::Message;
use tg_ai_bot_teloxide::telegram::entities::message_url_targets;

#[test]
fn message_adapter_extracts_caption_text_link_after_non_bmp_emoji() {
    let message: Message = serde_json::from_value(serde_json::json!({
        "message_id": 19,
        "date": 1_700_000_000,
        "chat": {"id": 42, "type": "private"},
        "from": {"id": 13, "is_bot": false, "first_name": "Synthetic"},
        "photo": [{
            "file_id": "synthetic-file",
            "file_unique_id": "synthetic-photo",
            "width": 1,
            "height": 1
        }],
        "caption": "😺open",
        "caption_entities": [{
            "type": "text_link",
            "offset": 2,
            "length": 4,
            "url": "https://hidden.example/private?token=synthetic"
        }]
    }))
    .unwrap();

    let targets = message_url_targets(&message);
    assert_eq!(targets.len(), 1);
    assert_eq!(
        targets[0].value,
        "https://hidden.example/private?token=synthetic"
    );
    assert_eq!(targets[0].label.as_deref(), Some("open"));
    assert_eq!(targets[0].span, Some(4..8));
    assert_eq!(targets[0].source, teloxide_url::Source::TelegramEntity);
}
