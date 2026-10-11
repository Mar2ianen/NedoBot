use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, http::Uri, routing::post};
use serde_json::{Value, json};
use sqlx::{PgPool, query, query_as, query_scalar};
use teloxide::Bot;
use tg_ai_bot_teloxide::{
    community::ChatRegistry,
    config::Config,
    features::{
        auto_moderation::{decide_enforcement_for_audit, maybe_enforce_audit},
        labels::{LabelSource, SpamLabel, record_not_spam, record_spam},
        new_user_analysis::enqueue_new_user_audit_for_profile_refresh,
        new_user_audit::repo::NewUserAuditJob,
    },
};

const CHAT_ID: i64 = -1_009_000_099_410;
const USER_ID: i64 = 9_000_099_410;

#[derive(Default)]
struct TelegramStub {
    calls: Mutex<Vec<String>>,
    member: Mutex<String>,
}

async fn telegram_api(
    State(stub): State<Arc<TelegramStub>>,
    uri: Uri,
    Json(payload): Json<Value>,
) -> Json<Value> {
    let method = uri.path().rsplit('/').next().unwrap().to_lowercase();
    stub.calls.lock().unwrap().push(method.clone());
    assert_eq!(payload["chat_id"], CHAT_ID);
    let user = json!({"id": USER_ID, "is_bot": false, "first_name": "Fixture"});
    let result = match method.as_str() {
        "getchatmember" => {
            assert_eq!(payload["user_id"], USER_ID);
            match stub.member.lock().unwrap().as_str() {
                "restricted" => json!({
                    "status": "restricted", "user": user, "is_member": true,
                    "until_date": 0, "can_send_messages": false,
                    "can_send_audios": false, "can_send_documents": false,
                    "can_send_photos": false, "can_send_videos": false,
                    "can_send_video_notes": false, "can_send_voice_notes": false,
                    "can_send_other_messages": false, "can_add_web_page_previews": false,
                    "can_send_polls": false, "can_change_info": false,
                    "can_invite_users": false, "can_pin_messages": false,
                    "can_manage_topics": false, "can_react_to_messages": false,
                    "can_edit_tag": false
                }),
                "banned" => json!({"status": "kicked", "until_date": 0, "user": user}),
                "creator" => json!({"status": "creator", "is_anonymous": false, "user": user}),
                _ => json!({"status": "member", "user": user}),
            }
        }
        "getchat" => json!({
            "id": CHAT_ID, "type": "supergroup", "title": "Fixture",
            "accent_color_id": 0, "max_reaction_count": 1,
            "accepted_gift_types": {"unlimited_gifts": false, "limited_gifts": false,
                "unique_gifts": false, "premium_subscription": false},
            "permissions": {"can_send_messages": true}
        }),
        "restrictchatmember" => {
            assert_eq!(payload["user_id"], USER_ID);
            assert!(
                payload["permissions"]
                    .as_object()
                    .unwrap()
                    .values()
                    .all(|value| value == false)
            );
            assert_eq!(payload["use_independent_chat_permissions"], true);
            *stub.member.lock().unwrap() = "restricted".into();
            json!(true)
        }
        "deletemessages" => {
            assert_eq!(payload["message_ids"], json!([801]));
            json!(true)
        }
        "sendmessage" => json!({
            "message_id": 900, "date": 1_700_000_000,
            "chat": {"id": CHAT_ID, "type": "supergroup"}, "text": "captcha"
        }),
        _ => panic!("unexpected Telegram method: {method}"),
    };
    Json(json!({"ok": true, "result": result}))
}

async fn load_job(pool: &PgPool) -> NewUserAuditJob {
    let (id, snapshot_hash, input_json): (i64, String, Value) = query_as(
        "select j.id,j.snapshot_hash,j.input_json from new_user_audit_jobs j join telegram_new_user_profile_audits a on a.chat_id = j.chat_id and a.telegram_user_id = j.telegram_user_id and a.unified_audit_snapshot_hash = j.snapshot_hash where j.chat_id = $1 and j.telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .unwrap();
    NewUserAuditJob {
        id,
        chat_id: CHAT_ID,
        telegram_user_id: USER_ID,
        snapshot_hash,
        prompt_version: "fixture".into(),
        input_json,
        avatar_file_id: None,
        avatar_file_unique_id: None,
        assessment_json: None,
        review_threshold: 70,
        attempts: 1,
        materialization_attempts: 1,
        is_materialization_replay: true,
    }
}

pub async fn assert_known_spammer_is_rechecked(pool: &PgPool) {
    let mut config = Config::from_env().unwrap();
    let chat = config.community.chats.get_mut("main").unwrap();
    chat.id = CHAT_ID;
    chat.moderation = true;
    config.chat_registry = ChatRegistry::new(&config.community).unwrap();
    config.community.moderation.captcha_enabled = true;
    config.community.moderation.captcha_dry_run = false;
    config.community.moderation.captcha_threshold = 85;
    config.community.moderation.enforce_enabled = false;
    query("insert into telegram_chat_users (chat_id,telegram_user_id,first_seen_at,member_status,is_present) values ($1,$2,now() - interval '14 days','member',true),($1 - 1,$2,now() - interval '14 days','member',true)")
        .bind(CHAT_ID).bind(USER_ID).execute(pool).await.unwrap();
    query("insert into telegram_messages (chat_id,message_id,user_id,text,created_at,raw_json) values ($1,801,$2,'Обычное обсуждение маршрута',now(),jsonb_build_object('date',extract(epoch from now())::bigint))")
        .bind(CHAT_ID).bind(USER_ID).execute(pool).await.unwrap();
    enqueue_new_user_audit_for_profile_refresh(pool, &config, CHAT_ID, USER_ID)
        .await
        .unwrap();
    let clean_job = load_job(pool).await;
    let clean_score: i32 = query_scalar("select risk_baseline_score from telegram_new_user_profile_audits where chat_id = $1 and telegram_user_id = $2")
        .bind(CHAT_ID).bind(USER_ID).fetch_one(pool).await.unwrap();

    record_spam(
        pool,
        CHAT_ID,
        USER_ID,
        &SpamLabel {
            subtype: "fixture".into(),
            source: LabelSource::OwnerManual,
            reason: "Synthetic confirmation".into(),
            evidence: json!({}),
            operator_id: None,
        },
        false,
    )
    .await
    .unwrap();
    enqueue_new_user_audit_for_profile_refresh(pool, &config, CHAT_ID, USER_ID)
        .await
        .unwrap();
    let job = load_job(pool).await;
    assert_ne!(
        job.snapshot_hash, clean_job.snapshot_hash,
        "label changes must create a new audit generation"
    );
    assert_eq!(job.input_json["risk"]["local_spammer_identity"], true);
    let (score, signals): (i32, Value) = query_as("select risk_baseline_score,risk_baseline_signals from telegram_new_user_profile_audits where chat_id = $1 and telegram_user_id = $2")
        .bind(CHAT_ID).bind(USER_ID).fetch_one(pool).await.unwrap();
    assert_eq!(score, (clean_score + 70).min(100));
    assert!(
        signals
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["label"] == "local_spammer_identity" && s["coefficient"] == 70)
    );
    let decision = decide_enforcement_for_audit(
        pool,
        CHAT_ID,
        USER_ID,
        teloxide_antispam::policy::AutoAction::Ban,
        false,
    )
    .await
    .unwrap();
    assert_eq!(decision.auto_ban_evidence, vec!["local_spammer_identity"]);
    enqueue_new_user_audit_for_profile_refresh(pool, &config, CHAT_ID - 1, USER_ID)
        .await
        .unwrap();
    let foreign: Value = query_scalar(
        "select input_json from new_user_audit_jobs where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID - 1)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(foreign["risk"]["local_spammer_identity"], false);

    let stub = Arc::new(TelegramStub::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new()
        .fallback(post(telegram_api))
        .with_state(stub.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let bot = Bot::new("fixture-token").set_api_url(format!("http://{address}/").parse().unwrap());

    // Import time is not message freshness: the original Telegram date wins.
    query("update telegram_messages set raw_json = jsonb_build_object('date',extract(epoch from now() - interval '2 days')::bigint) where chat_id = $1 and message_id = 801")
        .bind(CHAT_ID).execute(pool).await.unwrap();
    maybe_enforce_audit(&bot, pool, &config, &job, 100)
        .await
        .unwrap();
    assert!(stub.calls.lock().unwrap().is_empty());
    query("update telegram_messages set raw_json = jsonb_build_object('date',extract(epoch from now())::bigint) where chat_id = $1 and message_id = 801")
        .bind(CHAT_ID).execute(pool).await.unwrap();
    let mut stale_job = job.clone();
    stale_job.snapshot_hash = "superseded".into();
    maybe_enforce_audit(&bot, pool, &config, &stale_job, 100)
        .await
        .unwrap();
    assert!(stub.calls.lock().unwrap().is_empty());

    for status in ["banned", "creator"] {
        *stub.member.lock().unwrap() = status.into();
        maybe_enforce_audit(&bot, pool, &config, &job, 100)
            .await
            .unwrap();
        assert!(
            stub.calls
                .lock()
                .unwrap()
                .iter()
                .all(|method| method == "getchatmember")
        );
    }
    *stub.member.lock().unwrap() = "member".into();
    query("insert into spam_label_events (chat_id,telegram_user_id,label,subtype,source,reason,evidence) values ($1,$2,'spam','fixture','system','Historic system label','{}')")
        .bind(CHAT_ID).bind(USER_ID).execute(pool).await.unwrap();
    maybe_enforce_audit(&bot, pool, &config, &job, 100)
        .await
        .unwrap();
    let (state, score, expiry): (String, i32, chrono::DateTime<chrono::Utc>) = query_as("select status,risk_score,expires_at from telegram_risk_captcha_challenges where chat_id = $1 and telegram_user_id = $2")
        .bind(CHAT_ID).bind(USER_ID).fetch_one(pool).await.unwrap();
    assert_eq!(state, "pending");
    assert_eq!(score, 100);
    maybe_enforce_audit(&bot, pool, &config, &job, 100)
        .await
        .unwrap();
    assert_eq!(
        stub.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|m| *m == "sendmessage")
            .count(),
        1
    );
    let replay_expiry: chrono::DateTime<chrono::Utc> = query_scalar("select expires_at from telegram_risk_captcha_challenges where chat_id = $1 and telegram_user_id = $2")
        .bind(CHAT_ID).bind(USER_ID).fetch_one(pool).await.unwrap();
    assert_eq!(expiry, replay_expiry);

    record_not_spam(
        pool,
        CHAT_ID,
        USER_ID,
        "Synthetic correction",
        &json!({}),
        None,
    )
    .await
    .unwrap();
    enqueue_new_user_audit_for_profile_refresh(pool, &config, CHAT_ID, USER_ID)
        .await
        .unwrap();
    let corrected = load_job(pool).await;
    assert_eq!(
        corrected.input_json["risk"]["local_spammer_identity"],
        false
    );
    assert_ne!(corrected.snapshot_hash, job.snapshot_hash);
    let calls_before = stub.calls.lock().unwrap().len();
    maybe_enforce_audit(&bot, pool, &config, &corrected, 100)
        .await
        .unwrap();
    assert_eq!(calls_before, stub.calls.lock().unwrap().len());
    server.abort();
    for sql in [
        "delete from telegram_risk_captcha_challenges where telegram_user_id = $1",
        "delete from spam_review_requests where telegram_user_id = $1",
        "delete from new_user_audit_jobs where telegram_user_id = $1",
        "delete from telegram_new_user_profile_audits where telegram_user_id = $1",
        "delete from spam_label_events where telegram_user_id = $1",
        "delete from telegram_messages where user_id = $1",
        "delete from telegram_chat_users where telegram_user_id = $1",
    ] {
        query(sql).bind(USER_ID).execute(pool).await.unwrap();
    }
}
