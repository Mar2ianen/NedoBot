use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use axum::{Json, Router, extract::State, http::Uri, routing::post};
use serde_json::{Value, json};
use sqlx::{PgPool, query, query_scalar};
use teloxide::Bot;
use tg_ai_bot_teloxide::features::{
    new_user_audit::repo::NewUserAuditJob,
    risk_captcha::{
        CaptchaOutcome, CaptchaTriggerSource, apply_risk_captcha, apply_risk_captcha_with_source,
    },
};
use uuid::Uuid;

const CHAT_ID: i64 = -1_009_000_099_321;
const USER_ID: i64 = 9_000_099_321;

#[derive(Default)]
struct TelegramStub {
    calls: Mutex<Vec<(String, Value)>>,
    fail_delete: AtomicBool,
    restricted: AtomicBool,
}

async fn telegram_api(
    State(stub): State<Arc<TelegramStub>>,
    uri: Uri,
    Json(payload): Json<Value>,
) -> Json<Value> {
    let method = uri.path().rsplit('/').next().unwrap().to_lowercase();
    stub.calls.lock().unwrap().push((method.clone(), payload));
    let result = match method.as_str() {
        "getchatmember" if stub.restricted.load(Ordering::SeqCst) => json!({
            "status": "restricted", "is_member": true, "until_date": 0,
            "user": {"id": USER_ID, "is_bot": false, "first_name": "Fixture"},
            "can_send_messages": false, "can_send_audios": false,
            "can_send_documents": false, "can_send_photos": false,
            "can_send_videos": false, "can_send_video_notes": false,
            "can_send_voice_notes": false, "can_send_other_messages": false,
            "can_add_web_page_previews": false, "can_send_polls": false,
            "can_change_info": false, "can_invite_users": false,
            "can_pin_messages": false, "can_manage_topics": false,
            "can_react_to_messages": false, "can_edit_tag": false
        }),
        "getchatmember" => json!({
            "status": "member",
            "user": {"id": USER_ID, "is_bot": false, "first_name": "Fixture"}
        }),
        "deletemessages" if stub.fail_delete.swap(false, Ordering::SeqCst) => {
            return Json(json!({
                "ok": false, "error_code": 400,
                "description": "Bad Request: not enough rights to delete messages"
            }));
        }
        "deletemessages" => json!(true),
        "sendmessage" => json!({
            "message_id": 900, "date": 1_700_000_000,
            "chat": {"id": CHAT_ID, "type": "supergroup"}, "text": "captcha"
        }),
        _ => panic!("unexpected Telegram method: {method}"),
    };
    Json(json!({"ok": true, "result": result}))
}

fn delete_batches(stub: &TelegramStub) -> Vec<Vec<i64>> {
    stub.calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, _)| method == "deletemessages")
        .map(|(_, payload)| {
            assert_eq!(payload["chat_id"], CHAT_ID);
            payload["message_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_i64().unwrap())
                .collect()
        })
        .collect()
}

fn send_count(stub: &TelegramStub) -> usize {
    stub.calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, _)| method == "sendmessage")
        .count()
}

pub async fn assert_cleanup(pool: &PgPool) {
    query(
        r#"insert into telegram_new_user_profile_audits
             (chat_id, telegram_user_id, first_message_id, last_message_id,
              risk_score, risk_level, risk_signal_breakdown)
           values ($1, $2, 101, 109, 100, 'high', '[]')"#,
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .unwrap();
    query(
        r#"insert into telegram_messages
             (chat_id, message_id, user_id, text, has_sticker,
              source_channel_id, deleted_by_bot_at, created_at, raw_json)
           values
             ($1, 100, $2, 'before audit', false, null, null, now(), '{}'),
             ($1, 101, $2, 'spam one', false, null, null, now(), '{}'),
             ($1, 102, $2, 'spam two', false, null, null, now(), '{}'),
             ($1, 103, $2, null, true, null, null, now(), '{}'),
             ($1, 104, $2, 'already cleaned', false, null, now(), now(), '{}'),
             ($1, 105, $2, null, false, null, null, now(), '{"new_chat_members": []}'),
             ($1, 106, $2, 'channel post', false, 11, null, now(), '{}'),
             ($1, 107, $2 + 1, 'another user', false, null, null, now(), '{}'),
             ($1, 108, $2, 'stale message', false, null, null, now() - interval '2 days', '{}'),
             ($1, 109, $2, '   ', false, null, null, now(), '{}'),
             ($1, 400, $2, 'outside audit', false, null, null, now(), '{}'),
             ($1 - 1, 101, $2, 'another chat', false, null, null, now(), '{}')"#,
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .unwrap();
    let id = Uuid::new_v4();
    query(
        r#"insert into telegram_risk_captcha_challenges
             (id, chat_id, telegram_user_id, audit_job_id, risk_score, question,
              options, correct_option, restore_permissions, restriction_applied_at)
           values ($1, $2, $3, 1, 100, '2 + 2 = ?', '["3","4","5","6"]', 1,
                   '{"can_send_messages": true}', now())"#,
    )
    .bind(id)
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .unwrap();
    let job = NewUserAuditJob {
        id: 1,
        chat_id: CHAT_ID,
        telegram_user_id: USER_ID,
        snapshot_hash: "captcha-cleanup-fixture".into(),
        prompt_version: "fixture".into(),
        input_json: json!({}),
        avatar_file_id: None,
        avatar_file_unique_id: None,
        assessment_json: None,
        review_threshold: 70,
        attempts: 1,
        materialization_attempts: 1,
        is_materialization_replay: true,
    };
    let stub = Arc::new(TelegramStub::default());
    stub.fail_delete.store(true, Ordering::SeqCst);
    stub.restricted.store(true, Ordering::SeqCst);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new()
        .fallback(post(telegram_api))
        .with_state(stub.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let bot = Bot::new("fixture-token").set_api_url(format!("http://{address}/").parse().unwrap());

    assert!(
        apply_risk_captcha_with_source(
            &bot,
            pool,
            &job,
            100,
            600,
            CaptchaTriggerSource::PaidDmPlusOffer,
        )
        .await
        .is_err()
    );
    assert_eq!(delete_batches(&stub), vec![vec![103, 102, 101]]);
    assert_eq!(
        send_count(&stub),
        0,
        "failed cleanup must precede captcha delivery"
    );
    let state: (String, Option<String>, String) = sqlx::query_as(
        "select status, setup_error_kind, trigger_source from telegram_risk_captcha_challenges where id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        state,
        (
            "preparing".into(),
            Some("setup_attempt_failed".into()),
            "paid_dm_plus_offer_v1".into()
        )
    );
    let unmarked: i64 = query_scalar(
        "select count(*) from telegram_messages where chat_id = $1 and message_id between 101 and 103 and deleted_by_bot_at is null",
    )
    .bind(CHAT_ID)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        unmarked, 3,
        "failed Telegram batch must not mark rows as deleted"
    );
    query(
        "update telegram_risk_captcha_challenges set setup_next_attempt_at = now() where id = $1",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(
        apply_risk_captcha(&bot, pool, &job, 100, 600)
            .await
            .unwrap(),
        CaptchaOutcome::Issued
    );
    let trigger_source: String =
        query_scalar("select trigger_source from telegram_risk_captcha_challenges where id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(trigger_source, "paid_dm_plus_offer_v1");
    assert_eq!(send_count(&stub), 1);
    let expiry: chrono::DateTime<chrono::Utc> =
        query_scalar("select expires_at from telegram_risk_captcha_challenges where id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    let incorrectly_deleted: i64 = query_scalar(
        "select count(*) from telegram_messages where chat_id in ($1, $1 - 1) and deleted_by_bot_at is not null and message_id not in (101,102,103,104)",
    )
    .bind(CHAT_ID)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        incorrectly_deleted, 0,
        "cleanup must respect audit and sender scope"
    );
    let foreign_deleted: bool = query_scalar(
        "select deleted_by_bot_at is not null from telegram_messages where chat_id = $1 - 1 and message_id = 101",
    )
    .bind(CHAT_ID)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(
        !foreign_deleted,
        "same message id in a foreign chat must survive"
    );

    query(
        "insert into telegram_messages (chat_id, message_id, user_id, text) select $1, n, $2, 'late spam' from generate_series(200,300) n",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .unwrap();
    query("update telegram_new_user_profile_audits set last_message_id = 300 where chat_id = $1 and telegram_user_id = $2")
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(pool)
        .await
        .unwrap();
    let previous_batches = delete_batches(&stub).len();
    assert_eq!(
        apply_risk_captcha(&bot, pool, &job, 100, 600)
            .await
            .unwrap(),
        CaptchaOutcome::AlreadyPending
    );
    let batches = delete_batches(&stub);
    assert_eq!(batches.len() - previous_batches, 2);
    assert_eq!(batches[previous_batches].len(), 100);
    assert_eq!(batches[previous_batches + 1], vec![200]);
    assert_eq!(
        send_count(&stub),
        1,
        "pending captcha must not be sent twice"
    );
    let current_expiry: chrono::DateTime<chrono::Utc> =
        query_scalar("select expires_at from telegram_risk_captcha_challenges where id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        expiry, current_expiry,
        "cleanup must not prolong captcha TTL"
    );
    let before_repeat = delete_batches(&stub).len();
    apply_risk_captcha(&bot, pool, &job, 100, 600)
        .await
        .unwrap();
    assert_eq!(
        delete_batches(&stub).len(),
        before_repeat,
        "cleaned messages must be skipped"
    );

    stub.restricted.store(false, Ordering::SeqCst);
    assert_eq!(
        apply_risk_captcha(&bot, pool, &job, 100, 600)
            .await
            .unwrap(),
        CaptchaOutcome::Overridden
    );
    assert_eq!(delete_batches(&stub).len(), before_repeat);
    query("update telegram_risk_captcha_challenges set status = 'passed' where id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    let before_passed = stub.calls.lock().unwrap().len();
    assert_eq!(
        apply_risk_captcha(&bot, pool, &job, 100, 600)
            .await
            .unwrap(),
        CaptchaOutcome::AlreadyPassed
    );
    assert_eq!(stub.calls.lock().unwrap().len(), before_passed);
    server.abort();
    query("delete from telegram_risk_captcha_challenges where id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    query("delete from telegram_new_user_profile_audits where chat_id = $1")
        .bind(CHAT_ID)
        .execute(pool)
        .await
        .unwrap();
    query("delete from telegram_messages where chat_id in ($1, $1 - 1)")
        .bind(CHAT_ID)
        .execute(pool)
        .await
        .unwrap();
}
