// PostgreSQL-проверка local→review без LLM, Telegram и доверия старой метке.
use serde_json::{Value, json};
use sqlx::{PgPool, query, query_as, query_scalar};
use teloxide::Bot;
use tg_ai_bot_teloxide::{
    community::ChatRegistry,
    config::Config,
    features::{
        auto_moderation::maybe_enforce_audit,
        jobs::claim::CasResult,
        labels::{LabelSource, SpamLabel, record_not_spam, record_spam},
        new_user_audit::{
            local_cascade::{self, LocalCascadeMode},
            repo::{
                NewUserAuditJobParams, claim_next_new_user_audit_job, enqueue_new_user_audit_job,
                restart_revoked_local_assessment,
            },
            service::process_next_new_user_audit_job,
        },
    },
};
const CHAT: i64 = -1_009_911_000;
const TEXT: &str = "Нужны срочно деньги сегодня напишите мне в личные сообщения";
const SOURCE_A: i64 = 9_911_001;
const SOURCE_B: i64 = 9_911_002;
const CANDIDATE: i64 = 9_911_003;

async fn source(pool: &PgPool, chat: i64, user: i64) {
    query("insert into telegram_chat_users (chat_id,telegram_user_id) values ($1,$2)")
        .bind(chat)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    query("insert into telegram_messages (chat_id,message_id,user_id,text,created_at) values ($1,$2,$3,$4,now()-interval '1 hour')")
        .bind(chat).bind(user as i32).bind(user).bind(TEXT).execute(pool).await.unwrap();
    record_spam(
        pool,
        chat,
        user,
        &SpamLabel {
            subtype: "fixture".into(),
            source: LabelSource::OwnerManual,
            reason: "Synthetic label".into(),
            evidence: json!({}),
            operator_id: None,
        },
        false,
    )
    .await
    .unwrap();
}
async fn enqueue(pool: &PgPool, user: i64) -> i64 {
    query("insert into telegram_chat_users (chat_id,telegram_user_id,first_seen_at,member_status,is_present) values ($1,$2,now(),'member',true)")
        .bind(CHAT).bind(user).execute(pool).await.unwrap();
    query("insert into telegram_new_user_profile_audits (chat_id,telegram_user_id,risk_baseline_score,risk_baseline_signals,risk_external_score,risk_external_signals,risk_score,risk_level) values ($1,$2,12,'[{\"label\":\"fixture_baseline\"}]',3,'[{\"label\":\"fixture_external\"}]',15,'low')")
        .bind(CHAT).bind(user).execute(pool).await.unwrap();
    let input = json!({"deterministic_campaign_rule":null,"text":{"first_message_preview":"Привет всем участникам","recent_message_previews":[TEXT]},"activity":{"message_count":2,"chat_age_sec":60}});
    enqueue_new_user_audit_job(
        pool,
        NewUserAuditJobParams {
            chat_id: CHAT,
            telegram_user_id: user,
            snapshot_hash: &format!("local-cascade-{user}"),
            prompt_version: "fixture",
            input_json: &input,
            avatar_file_id: Some("must-not-download"),
            avatar_file_unique_id: Some("fixture"),
            review_threshold: 70,
        },
    )
    .await
    .unwrap();
    query_scalar("select id from new_user_audit_jobs where chat_id=$1 and telegram_user_id=$2")
        .bind(CHAT)
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap()
}
pub async fn assert_local_cascade(pool: &PgPool) {
    let mut config = Config::from_env().unwrap();
    let chat = config.community.chats.get_mut("main").unwrap();
    chat.id = CHAT;
    chat.moderation = true;
    config.chat_registry = ChatRegistry::new(&config.community).unwrap();
    config.community.moderation.enabled = true;
    config.community.moderation.local_cascade.mode = LocalCascadeMode::ReviewOnly;
    config.community.moderation.enforce_enabled = true;
    config.community.moderation.enforce_dry_run = false;
    config.community.moderation.captcha_enabled = true;
    config.community.moderation.captcha_dry_run = false;
    config.community.moderation.cas_enabled = true; // Local branch must not reach CAS.
    config.linear_spam_model = None;
    config.llm_profiles = None; // Any accidental LLM route fails.
    let bot = Bot::new("fixture-token").set_api_url("http://127.0.0.1:9".parse().unwrap());
    source(pool, CHAT, SOURCE_A).await;
    source(pool, CHAT, SOURCE_B).await;
    source(pool, CHAT - 1, SOURCE_A).await;
    let corpus = local_cascade::load_corpus(pool, CHAT, 100).await.unwrap();
    assert_eq!(corpus.len(), 2);
    assert!(corpus.iter().all(|e| e.chat_id == CHAT));
    let id = enqueue(pool, CANDIDATE).await;
    assert!(
        process_next_new_user_audit_job(&bot, pool, &config)
            .await
            .unwrap()
    );
    let (provider, assessment): (String, Value) =
        query_as("select provider,assessment_json from new_user_audit_jobs where id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(provider, "local_cascade");
    let evidence = local_cascade::metadata(&assessment).unwrap().unwrap();
    assert_eq!(
        evidence.message_slot,
        Some(1),
        "recent spam after a greeting must be evaluated"
    );
    assert!(evidence.applied);
    assert!(
        process_next_new_user_audit_job(&bot, pool, &config)
            .await
            .unwrap()
    );
    let (status,score,baseline,external,signals):(String,i32,i32,i32,Value)=query_as(
        "select j.materialization_status,a.risk_score,a.risk_baseline_score,a.risk_external_score,a.risk_first_message_signals from new_user_audit_jobs j join telegram_new_user_profile_audits a using(chat_id,telegram_user_id) where j.id=$1"
    ).bind(id).fetch_one(pool).await.unwrap();
    assert_eq!(
        (status.as_str(), score, baseline, external),
        ("succeeded", 70, 12, 3)
    );
    assert_eq!(signals[0]["auto_ban_eligible"], false);
    let labels: i64 = query_scalar(
        "select count(*) from spam_label_events where chat_id=$1 and telegram_user_id=$2",
    )
    .bind(CHAT)
    .bind(CANDIDATE)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(labels, 0);
    let reviews:i64=query_scalar("select count(*) from spam_review_requests where chat_id=$1 and telegram_user_id=$2 and status='pending'").bind(CHAT).bind(CANDIDATE).fetch_one(pool).await.unwrap();
    assert_eq!(reviews, 1);
    // Старый worker не должен оживить отозванное локальное решение.
    let revoked_id = enqueue(pool, CANDIDATE + 1).await;
    assert!(
        process_next_new_user_audit_job(&bot, pool, &config)
            .await
            .unwrap()
    );
    let old_claim = claim_next_new_user_audit_job(pool).await.unwrap().unwrap();
    assert_eq!(old_claim.id, revoked_id);
    maybe_enforce_audit(&bot, pool, &config, &old_claim, 100)
        .await
        .unwrap();
    record_not_spam(
        pool,
        CHAT,
        SOURCE_B,
        "Synthetic correction",
        &json!({}),
        None,
    )
    .await
    .unwrap();
    assert!(
        local_cascade::load_corpus(pool, CHAT, 100)
            .await
            .unwrap()
            .iter()
            .any(|e| e.user_id == SOURCE_B && e.label == "not_spam")
    );
    query("update new_user_audit_jobs set materialization_lease_expires_at=now()-interval '1 second' where id=$1")
        .bind(revoked_id).execute(pool).await.unwrap();
    assert!(
        process_next_new_user_audit_job(&bot, pool, &config)
            .await
            .unwrap()
    );
    let (status,assessment,generation,materialization):(String,Option<Value>,i64,i64)=query_as("select status,assessment_json,generation_fence,materialization_fence from new_user_audit_jobs where id=$1")
        .bind(revoked_id).fetch_one(pool).await.unwrap();
    assert_eq!(status, "retry_wait");
    assert!(assessment.is_none());
    assert!(generation > old_claim.generation_fence);
    assert!(materialization > old_claim.materialization_fence);
    assert_ne!(
        restart_revoked_local_assessment(pool, &old_claim)
            .await
            .unwrap(),
        CasResult::Applied
    );
    let score:i32=query_scalar("select risk_score from telegram_new_user_profile_audits where chat_id=$1 and telegram_user_id=$2").bind(CHAT).bind(CANDIDATE+1).fetch_one(pool).await.unwrap();
    assert_eq!(score, 15);
    // Сообщение, отредактированное после подтверждения автора, не считается проверенным.
    query("update telegram_messages set edited_at=now()+interval '1 second' where chat_id=$1 and user_id=$2").bind(CHAT).bind(SOURCE_A).execute(pool).await.unwrap();
    assert!(
        !local_cascade::load_corpus(pool, CHAT, 100)
            .await
            .unwrap()
            .iter()
            .any(|e| e.user_id == SOURCE_A)
    );
    // Более новая system-метка не должна превращать старый manual stamp в gold.
    query("insert into spam_label_events (chat_id,telegram_user_id,label,subtype,source,reason,evidence) values ($1,$2,'spam','fixture','system','Synthetic automatic label','{}')")
        .bind(CHAT).bind(SOURCE_B).execute(pool).await.unwrap();
    assert!(
        local_cascade::load_corpus(pool, CHAT, 100)
            .await
            .unwrap()
            .is_empty()
    );
    for sql in [
        "delete from spam_review_requests where chat_id=any($1)",
        "delete from new_user_audit_jobs where chat_id=any($1)",
        "delete from telegram_new_user_profile_audits where chat_id=any($1)",
        "delete from spam_label_events where chat_id=any($1)",
        "delete from telegram_messages where chat_id=any($1)",
        "delete from telegram_chat_users where chat_id=any($1)",
    ] {
        query(sql)
            .bind([CHAT, CHAT - 1])
            .execute(pool)
            .await
            .unwrap();
    }
}
