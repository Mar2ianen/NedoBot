use sqlx::{PgPool, query, query_as, query_scalar};
use tg_ai_bot_teloxide::{
    config::Config,
    db::telegram::{save_edited_telegram_message, save_telegram_message},
    telegram::entities::MessageRevision,
};

const MESSAGE_ID: i32 = 9_410_001;
const JOIN_MESSAGE_ID: i32 = MESSAGE_ID + 1;
const USER_ID: i64 = 9_410_001;
const BOT_ID: i64 = 9_410_002;
const JOINED_USER_ID: i64 = 9_410_003;

pub async fn assert_message_persistence_effects(pool: &PgPool) {
    let mut config = Config::from_env().expect("test community config must parse");
    let chat_id = config
        .community
        .chats
        .values()
        .next()
        .expect("test community has a managed chat")
        .id;
    config.chat_retrieval_embeddings_enabled = true;
    config.community.moderation.qr_evidence_enabled = false;

    cleanup(pool, chat_id).await;

    let original = message(
        chat_id,
        MESSAGE_ID,
        USER_ID,
        "open",
        Some("https://example.org/hidden"),
    );
    let first = save_telegram_message(pool, &original, &config)
        .await
        .expect("first delivery must persist");
    assert!(first.inserted);
    assert_eq!(first.message.chat_id, chat_id);
    assert_eq!(first.message.message_id, MESSAGE_ID);
    assert_eq!(first.revision, MessageRevision::Original);

    let duplicate = save_telegram_message(pool, &original, &config)
        .await
        .expect("duplicate update must be safely upserted");
    assert!(!duplicate.inserted);
    assert_eq!(duplicate.message, first.message);
    assert_eq!(duplicate.revision, first.revision);

    let message_count: i64 = query_scalar(
        "select message_count from telegram_chat_users where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(chat_id)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("activity counter must be materialized");
    assert_eq!(
        message_count, 1,
        "duplicate delivery must not increment activity twice"
    );

    let embedding_count: i64 = query_scalar(
        "select count(*) from telegram_message_embeddings_gemma2 where chat_id = $1 and message_id = $2",
    )
    .bind(chat_id)
    .bind(MESSAGE_ID)
    .fetch_one(pool)
    .await
    .expect("embedding queue must be queryable");
    assert_eq!(
        embedding_count, 1,
        "duplicate delivery must enqueue one embedding"
    );

    let evidence_count: i64 = query_scalar(
        "select count(*) from telegram_message_url_evidence where chat_id = $1 and message_id = $2",
    )
    .bind(chat_id)
    .bind(MESSAGE_ID)
    .fetch_one(pool)
    .await
    .expect("URL evidence must be queryable");
    assert_eq!(
        evidence_count, 1,
        "hidden text-link target must be materialized"
    );

    let edited: teloxide::types::Message = serde_json::from_value(serde_json::json!({
        "message_id": MESSAGE_ID,
        "date": 1_700_000_000,
        "edit_date": 1_700_000_100,
        "chat": {"id": chat_id, "type": "supergroup"},
        "from": {"id": USER_ID, "is_bot": false, "first_name": "Synthetic user"},
        "text": "edited without a link"
    }))
    .expect("edited synthetic Telegram message must deserialize");
    let edit = save_edited_telegram_message(pool, &edited, &config)
        .await
        .expect("edit must update the persisted projection");
    assert!(!edit.inserted);
    assert_eq!(
        edit.revision,
        MessageRevision::Edited {
            edit_date_unix: Some(1_700_000_100)
        }
    );

    let evidence_after_edit: i64 = query_scalar(
        "select count(*) from telegram_message_url_evidence where chat_id = $1 and message_id = $2",
    )
    .bind(chat_id)
    .bind(MESSAGE_ID)
    .fetch_one(pool)
    .await
    .expect("edited URL evidence must be queryable");
    assert_eq!(
        evidence_after_edit, 0,
        "editing away a link must clear stale evidence"
    );

    let (edit_count, stored_text): (i32, Option<String>) = query_as(
        "select edit_count, text from telegram_messages where chat_id = $1 and message_id = $2",
    )
    .bind(chat_id)
    .bind(MESSAGE_ID)
    .fetch_one(pool)
    .await
    .expect("edited message row must remain available");
    assert_eq!(edit_count, 1);
    assert_eq!(stored_text.as_deref(), Some("edited without a link"));

    let membership_event = membership_message(chat_id);
    save_telegram_message(pool, &membership_event, &config)
        .await
        .expect("new-member service message must persist");
    let joined: bool = query_scalar(
        "select membership_first_confirmed_at is not null from telegram_chat_users where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(chat_id)
    .bind(JOINED_USER_ID)
    .fetch_one(pool)
    .await
    .expect("service-message membership evidence must exist");
    assert!(joined);

    cleanup(pool, chat_id).await;
}

fn message(
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    label: &str,
    target: Option<&str>,
) -> teloxide::types::Message {
    let mut value = serde_json::json!({
        "message_id": message_id,
        "date": 1_700_000_000,
        "chat": {"id": chat_id, "type": "supergroup"},
        "from": {"id": user_id, "is_bot": false, "first_name": "Synthetic user"},
        "text": label
    });
    if let Some(target) = target {
        value["entities"] = serde_json::json!([{
            "type": "text_link", "offset": 0, "length": label.encode_utf16().count(), "url": target
        }]);
    }
    serde_json::from_value(value).expect("synthetic Telegram message must deserialize")
}

fn membership_message(chat_id: i64) -> teloxide::types::Message {
    serde_json::from_value(serde_json::json!({
        "message_id": JOIN_MESSAGE_ID,
        "date": 1_700_000_010,
        "chat": {"id": chat_id, "type": "supergroup"},
        "from": {"id": BOT_ID, "is_bot": true, "first_name": "Synthetic bot"},
        "new_chat_members": [{"id": JOINED_USER_ID, "is_bot": false, "first_name": "New synthetic user"}]
    }))
    .expect("synthetic member-service Telegram message must deserialize")
}

async fn cleanup(pool: &PgPool, chat_id: i64) {
    query("delete from telegram_message_embeddings_gemma2 where chat_id = $1 and message_id between $2 and $3")
        .bind(chat_id).bind(MESSAGE_ID).bind(JOIN_MESSAGE_ID).execute(pool).await.unwrap();
    query("delete from telegram_messages where chat_id = $1 and message_id between $2 and $3")
        .bind(chat_id)
        .bind(MESSAGE_ID)
        .bind(JOIN_MESSAGE_ID)
        .execute(pool)
        .await
        .unwrap();
    query("delete from telegram_chat_users where chat_id = $1 and telegram_user_id = any($2)")
        .bind(chat_id)
        .bind(vec![USER_ID, BOT_ID, JOINED_USER_ID])
        .execute(pool)
        .await
        .unwrap();
    query("delete from telegram_user_profiles where telegram_user_id = any($1)")
        .bind(vec![USER_ID, BOT_ID, JOINED_USER_ID])
        .execute(pool)
        .await
        .unwrap();
}
