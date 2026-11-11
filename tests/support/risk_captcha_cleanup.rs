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
        CaptchaCallbackOutcome, CaptchaOutcome, apply_risk_captcha, handle_callback,
        process_next_expired, process_next_setup,
    },
};
use uuid::Uuid;

const CHAT_ID: i64 = -1_009_000_099_321;
const USER_ID: i64 = 9_000_099_321;

async fn reset_commenter(pool: &PgPool) {
    query("delete from telegram_risk_captcha_challenges where chat_id=$1")
        .bind(CHAT_ID)
        .execute(pool)
        .await
        .unwrap();
    query(
        "update telegram_messages set deleted_by_bot_at=null where chat_id=$1 and message_id=103",
    )
    .bind(CHAT_ID)
    .execute(pool)
    .await
    .unwrap();
}

async fn assert_non_member_reply(
    pool: &PgPool,
    bot: &Bot,
    job: &NewUserAuditJob,
    stub: &TelegramStub,
) {
    reset_commenter(pool).await;
    stub.non_member.store(true, Ordering::SeqCst);
    stub.calls.lock().unwrap().clear();
    assert_eq!(
        apply_risk_captcha(bot, pool, job, 100, 600).await.unwrap(),
        CaptchaOutcome::Issued
    );
    let (id,option,unrestricted):(Uuid,i16,bool)=sqlx::query_as("select id,correct_option,restriction_applied_at is null from telegram_risk_captcha_challenges where chat_id=$1")
        .bind(CHAT_ID).fetch_one(pool).await.unwrap();
    assert!(unrestricted);
    {
        let calls = stub.calls.lock().unwrap();
        assert!(!calls.iter().any(|(m, _)| matches!(
            m.as_str(),
            "restrictchatmember" | "banchatmember" | "unbanchatmember"
        )));
        let sent = calls.iter().find(|(m, _)| m == "sendmessage").unwrap();
        assert_eq!(sent.1["reply_parameters"]["message_id"], 103);
        assert_eq!(
            sent.1["ephemeral_message_parameters"]["receiver_user_id"],
            USER_ID
        );
    }
    let data = format!("rc:{id}:{option}");
    assert_eq!(
        handle_callback(bot, pool, Some(CHAT_ID - 1), USER_ID, &data)
            .await
            .unwrap(),
        CaptchaCallbackOutcome::Stale
    );
    assert_eq!(
        handle_callback(bot, pool, Some(CHAT_ID), USER_ID + 1, &data)
            .await
            .unwrap(),
        CaptchaCallbackOutcome::NotForThisUser
    );
    assert_eq!(
        handle_callback(bot, pool, Some(CHAT_ID), USER_ID, &data)
            .await
            .unwrap(),
        CaptchaCallbackOutcome::PassedWithoutRestriction
    );

    // Отклонённая отправка сохраняет комментарий, не запускает TTL и публичный fallback.
    reset_commenter(pool).await;
    stub.calls.lock().unwrap().clear();
    for _ in 0..5 {
        stub.fail_send.store(true, Ordering::SeqCst);
        query("update telegram_risk_captcha_challenges set setup_next_attempt_at=now() where chat_id=$1")
            .bind(CHAT_ID).execute(pool).await.unwrap();
        assert!(apply_risk_captcha(bot, pool, job, 100, 600).await.is_err());
    }
    let state:(String,bool,bool)=sqlx::query_as("select status,expires_at is null,restriction_applied_at is null from telegram_risk_captcha_challenges where chat_id=$1")
        .bind(CHAT_ID).fetch_one(pool).await.unwrap();
    assert_eq!(state, ("setup_failed".into(), true, true));
    {
        let calls = stub.calls.lock().unwrap();
        assert_eq!(calls.iter().filter(|(m, _)| m == "sendmessage").count(), 5);
        assert!(!calls.iter().any(|(m, _)| matches!(
            m.as_str(),
            "deletemessages" | "restrictchatmember" | "banchatmember" | "unbanchatmember"
        )));
        assert!(
            calls
                .iter()
                .filter(|(m, _)| m == "sendmessage")
                .all(|(_, p)| p["ephemeral_message_parameters"]["receiver_user_id"] == USER_ID)
        );
    }
    let preserved:bool=query_scalar("select deleted_by_bot_at is null from telegram_messages where chat_id=$1 and message_id=103")
        .bind(CHAT_ID).fetch_one(pool).await.unwrap();
    assert!(preserved);

    // Ответ без ephemeral ID не подтверждает отправку персональной капчи.
    reset_commenter(pool).await;
    stub.calls.lock().unwrap().clear();
    stub.omit_ephemeral_id.store(true, Ordering::SeqCst);
    assert!(apply_risk_captcha(bot, pool, job, 100, 600).await.is_err());
    let no_timer: bool = query_scalar(
        "select expires_at is null from telegram_risk_captcha_challenges where chat_id=$1",
    )
    .bind(CHAT_ID)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(no_timer);
    assert!(delete_batches(stub).is_empty());
    stub.omit_ephemeral_id.store(false, Ordering::SeqCst);

    // Истечение проверки non-member не меняет членство и не вызывает ban/unban.
    reset_commenter(pool).await;
    stub.calls.lock().unwrap().clear();
    assert_eq!(
        apply_risk_captcha(bot, pool, job, 100, 600).await.unwrap(),
        CaptchaOutcome::Issued
    );
    query("update telegram_risk_captcha_challenges set expires_at=now()-interval '1 second' where chat_id=$1")
        .bind(CHAT_ID).execute(pool).await.unwrap();
    assert!(process_next_expired(bot, pool).await.unwrap());
    let status: String =
        query_scalar("select status from telegram_risk_captcha_challenges where chat_id=$1")
            .bind(CHAT_ID)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(status, "expired");
    assert!(!stub.calls.lock().unwrap().iter().any(|(m, _)| matches!(
        m.as_str(),
        "restrictchatmember" | "banchatmember" | "unbanchatmember"
    )));

    // Без своего комментария нельзя создать проверку по сообщению чужого чата/автора.
    query("delete from telegram_risk_captcha_challenges where chat_id=$1")
        .bind(CHAT_ID)
        .execute(pool)
        .await
        .unwrap();
    stub.calls.lock().unwrap().clear();
    assert_eq!(
        apply_risk_captcha(bot, pool, job, 100, 600).await.unwrap(),
        CaptchaOutcome::Ineligible
    );
    assert_eq!(send_count(stub), 0);
}

#[derive(Default)]
struct TelegramStub {
    calls: Mutex<Vec<(String, Value)>>,
    fail_delete: AtomicBool,
    fail_send: AtomicBool,
    non_member: AtomicBool,
    omit_ephemeral_id: AtomicBool,
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
        "getchatmember" if stub.non_member.load(Ordering::SeqCst) => json!({
            "status": "left",
            "user": {"id": USER_ID, "is_bot": false, "first_name": "Fixture"}
        }),
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
        "sendmessage" if stub.fail_send.swap(false, Ordering::SeqCst) => {
            return Json(json!({"ok": false, "error_code":400,
                "description":"Bad Request: reply message not found"}));
        }
        "sendmessage" => {
            let mut message = json!({
                "message_id": 900, "date": 1_700_000_000,
                "chat": {"id": CHAT_ID, "type": "supergroup"}, "text": "captcha"
            });
            if !stub.omit_ephemeral_id.load(Ordering::SeqCst) {
                message["ephemeral_message_id"] = json!(901);
            }
            message
        }
        "getchat" => {
            json!({"id":CHAT_ID,"type":"supergroup","title":"Fixture", "accent_color_id":0,"max_reaction_count":1,
                "accepted_gift_types":{"unlimited_gifts":false,"limited_gifts":false,"unique_gifts":false,"premium_subscription":false},
                "permissions":{"can_send_messages":true}})
        }
        "deleteephemeralmessage" => json!(true),
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
        generation_fence: 1,
        materialization_attempts: 1,
        materialization_fence: 1,
        audit_generation: 0,
        is_materialization_replay: true,
    };
    let stub = Arc::new(TelegramStub::default());
    stub.fail_send.store(true, Ordering::SeqCst);
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
        apply_risk_captcha(&bot, pool, &job, 100, 600)
            .await
            .is_err()
    );
    assert!(delete_batches(&stub).is_empty());
    assert_eq!(
        send_count(&stub),
        1,
        "failed delivery must preserve the comment reply target"
    );
    let state: (String, Option<String>) = sqlx::query_as(
        "select status, setup_error_kind from telegram_risk_captcha_challenges where id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        state,
        ("preparing".into(), Some("setup_attempt_failed".into()))
    );
    let no_timer: bool =
        query_scalar("select expires_at is null from telegram_risk_captcha_challenges where id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(no_timer, "rejected ephemeral reply must not start expiry");
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
    assert_eq!(send_count(&stub), 2);
    assert_eq!(delete_batches(&stub), vec![vec![103, 102, 101]]);
    {
        let calls = stub.calls.lock().unwrap();
        let first_delete = calls
            .iter()
            .position(|(m, _)| m == "deletemessages")
            .unwrap();
        assert!(
            calls[..first_delete]
                .iter()
                .any(|(m, _)| m == "sendmessage")
        );
        for (_, payload) in calls.iter().filter(|(m, _)| m == "sendmessage") {
            assert_eq!(
                payload["ephemeral_message_parameters"]["receiver_user_id"],
                USER_ID
            );
            assert_eq!(payload["reply_parameters"]["message_id"], 103);
            assert_ne!(
                payload["reply_parameters"]["allow_sending_without_reply"],
                true
            );
        }
    }
    // После ошибки удаления повторяем только cleanup, сохраняя карточку и TTL.
    let initial_expiry: chrono::DateTime<chrono::Utc> =
        query_scalar("select expires_at from telegram_risk_captcha_challenges where id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    query("update telegram_risk_captcha_challenges set setup_next_attempt_at=now() where id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    assert!(process_next_setup(&bot, pool, 600).await.unwrap());
    assert_eq!(send_count(&stub), 2);
    let expiry: chrono::DateTime<chrono::Utc> =
        query_scalar("select expires_at from telegram_risk_captcha_challenges where id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        initial_expiry, expiry,
        "worker cleanup must preserve expiry"
    );
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
        2,
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
    assert_non_member_reply(pool, &bot, &job, &stub).await;
    server.abort();
    query("delete from telegram_risk_captcha_challenges where chat_id = $1")
        .bind(CHAT_ID)
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
