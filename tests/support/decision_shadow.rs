//! Реальные PostgreSQL ACL, scope, immutable snapshot, lease и veto для shadow.
use serde_json::Value;
use sqlx::{PgPool, query, query_scalar};
use tg_ai_bot_teloxide::{
    community::ChatRegistry,
    config::Config,
    features::{
        decision_audit::{
            client::{DecisionAssessment, Failure},
            repo,
            service::observe_message,
        },
        labels::record_not_spam,
    },
};
const CHAT: i64 = -1_009_912_000;
const USER: i64 = 9_912_001;
const TEXT: &str = "Есть П О Д Р А Б О Т К А, ПИШИ МНЕ!";
async fn source(pool: &PgPool, user: i64, message: i32) {
    query("insert into telegram_chat_users (chat_id,telegram_user_id,first_seen_at,message_count) values ($1,$2,now(),1) on conflict do nothing")
        .bind(CHAT).bind(user).execute(pool).await.unwrap();
    query("insert into telegram_messages (chat_id,message_id,user_id,text,created_at) values ($1,$2,$3,$4,now())")
        .bind(CHAT).bind(message).bind(user).bind(TEXT).execute(pool).await.unwrap();
}
fn config() -> Config {
    let mut c = Config::from_env().unwrap();
    let chat = c.community.chats.get_mut("main").unwrap();
    chat.id = CHAT;
    chat.ingest = true;
    chat.moderation = true;
    c.community.instance.id = "decision_fixture".into();
    c.chat_registry = ChatRegistry::new(&c.community).unwrap();
    c.community.moderation.enabled = true;
    c.community.moderation.decision.enabled = true;
    c.community.moderation.decision.external_processing_allowed = true;
    c.community.moderation.decision.max_calls_per_hour = 30;
    c
}
fn assessment() -> DecisionAssessment {
    serde_json::from_str(include_str!("../fixtures/decision_response.json")).unwrap()
}
pub async fn assert_decision_shadow(pool: &PgPool) {
    let mut c = config();
    source(pool, USER, 1).await;
    c.community.moderation.decision.enabled = false;
    observe_message(pool, &c, CHAT, 1).await.unwrap();
    assert!(repo::claim(pool, &c).await.unwrap().is_none());
    let count: i64 = query_scalar("select count(*) from moderation_decision_jobs")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    c.community.moderation.decision.enabled = true;
    observe_message(pool, &c, CHAT - 1, 1).await.unwrap(); // Не наш scope.
    observe_message(pool, &c, CHAT, 1).await.unwrap();
    observe_message(pool, &c, CHAT, 1).await.unwrap();
    let count: i64 = query_scalar("select count(*) from moderation_decision_jobs")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let first = repo::claim(pool, &c).await.unwrap().unwrap();
    assert!(first.input_json.get("user_id").is_none());
    assert!(first.input_json.get("reply_parent").is_none());
    assert!(repo::claim(pool, &c).await.unwrap().is_none());
    // Потеря lease: старый ответ не может перезаписать новый claim.
    query("update moderation_decision_jobs set lease_expires_at=now()-interval '1 second' where id=$1").bind(first.id).execute(pool).await.unwrap();
    let second = repo::claim(pool, &c).await.unwrap().unwrap();
    assert!(second.fence > first.fence);
    assert!(
        !repo::complete(pool, &first, &assessment(), 1)
            .await
            .unwrap()
    );
    assert!(!repo::fail(pool, &first, Failure::Http(400)).await.unwrap());
    assert!(
        repo::complete(pool, &second, &assessment(), 2)
            .await
            .unwrap()
    );
    let state: (String, Option<Value>) =
        sqlx::query_as("select status,assessment_json from moderation_decision_jobs where id=$1")
            .bind(second.id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(state.0, "succeeded");
    assert!(state.1.is_some());
    // Правка инвалидирует исходный снимок; санкции/review не появляются.
    source(pool, USER + 1, 2).await;
    observe_message(pool, &c, CHAT, 2).await.unwrap();
    let job = repo::claim(pool, &c).await.unwrap().unwrap();
    query("update telegram_messages set text='Теперь другой текст',edited_at=now() where chat_id=$1 and message_id=2").bind(CHAT).execute(pool).await.unwrap();
    assert!(!repo::complete(pool, &job, &assessment(), 1).await.unwrap());
    // Последний ручной normal veto до и во время обработки.
    source(pool, USER + 2, 3).await;
    observe_message(pool, &c, CHAT, 3).await.unwrap();
    let job = repo::claim(pool, &c).await.unwrap().unwrap();
    record_not_spam(
        pool,
        CHAT,
        USER + 2,
        "fixture",
        &serde_json::json!({}),
        None,
    )
    .await
    .unwrap();
    assert!(!repo::complete(pool, &job, &assessment(), 1).await.unwrap());
    observe_message(pool, &c, CHAT, 3).await.unwrap();
    assert!(repo::claim(pool, &c).await.unwrap().is_none());
    // Бюджет устойчив к retry и параллельным claims.
    source(pool, USER + 3, 4).await;
    source(pool, USER + 4, 5).await;
    observe_message(pool, &c, CHAT, 4).await.unwrap();
    observe_message(pool, &c, CHAT, 5).await.unwrap();
    query("update moderation_decision_budget set calls=$2 where instance_id=$1")
        .bind(&c.community.instance.id)
        .bind(c.community.moderation.decision.max_calls_per_hour - 1)
        .execute(pool)
        .await
        .unwrap();
    let (a, b) = tokio::join!(repo::claim(pool, &c), repo::claim(pool, &c));
    let jobs: Vec<_> = [a.unwrap(), b.unwrap()].into_iter().flatten().collect();
    assert_eq!(jobs.len(), 1);
    repo::fail(pool, &jobs[0], Failure::Http(400))
        .await
        .unwrap();
    let calls: i32 =
        query_scalar("select calls from moderation_decision_budget where instance_id=$1")
            .bind(&c.community.instance.id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(calls, 30);
    // Eligibility повторяется перед внешним запросом, а не только во время ingest.
    source(pool, USER + 5, 6).await;
    observe_message(pool, &c, CHAT, 6).await.unwrap();
    query(
        "update telegram_chat_users set message_count=11 where chat_id=$1 and telegram_user_id=$2",
    )
    .bind(CHAT)
    .bind(USER + 5)
    .execute(pool)
    .await
    .unwrap();
    assert!(repo::claim(pool, &c).await.unwrap().is_none());
    let status:String=query_scalar("select status from moderation_decision_jobs where instance_id=$1 and chat_id=$2 and message_id=6")
        .bind(&c.community.instance.id).bind(CHAT).fetch_one(pool).await.unwrap();
    assert_eq!(status, "stale");
    // Другой instance не видит/не claims очередь предыдущего.
    c.community.instance.id = "other_fixture".into();
    assert!(repo::claim(pool, &c).await.unwrap().is_none());
    c.community.instance.id = "decision_fixture".into();
    assert_first_author_text_sampling(pool, &c).await;
    assert_short_bait_profile_gate(pool, &c).await;
    assert_temporal_id_context_snapshot(pool, &c).await;
    assert_post_topic_provenance(pool, &c).await;
    // Новые таблицы недоступны настоящей MCP роли и не входят в public views.
    let mut tx = pool.begin().await.unwrap();
    query("set local role nedobot_mcp_ro")
        .execute(&mut *tx)
        .await
        .unwrap();
    let readable:bool=query_scalar("select has_table_privilege(current_user,'public.moderation_decision_jobs','SELECT') or has_table_privilege(current_user,'public.moderation_text_observations','SELECT') or has_table_privilege(current_user,'public.moderation_decision_budget','SELECT')").fetch_one(&mut *tx).await.unwrap();
    assert!(!readable);
    tx.rollback().await.unwrap();
    let reviews: i64 = query_scalar("select count(*) from spam_review_requests where chat_id=$1")
        .bind(CHAT)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(reviews, 0);
    let audits: i64 = query_scalar("select count(*) from new_user_audit_jobs where chat_id=$1")
        .bind(CHAT)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(audits, 0);
}

/// Контрпример старой ветке: второй короткий текст при initial_texts=1.
/// Проверяем реальный SQL enqueue, privacy и veto; внешние API не вызываются.
async fn assert_short_bait_profile_gate(pool: &PgPool, config: &Config) {
    for (index, (text, name, avatar, avatar_enabled, veto, expected)) in [
        ("Приму", true, false, false, "none", true),
        ("Приму", false, false, false, "none", false),
        ("Приму", false, true, true, "none", true),
        ("Приму", false, true, false, "none", false),
        ("Приму PR после тестов", true, true, true, "none", false),
        ("Прuму", true, false, false, "none", false),
        ("Пpиму\u{200b} к себе!!!", true, false, false, "none", true),
        ("Хороший день", true, true, true, "none", false),
        ("Приму", true, true, true, "admin", false),
        ("Приму", true, true, true, "bot", false),
        ("Приму", true, true, true, "normal", false),
        ("Приму", true, true, true, "old", false),
        ("Приму", true, true, true, "count", false),
        ("Приму", true, true, true, "disabled", false),
        ("Приму", true, true, true, "external", false),
        (
            "\u{200b}Котики, мне скучно, кто в гости? 😘",
            true,
            false,
            false,
            "none",
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let user = USER + 100 + index as i64;
        let prior = 1000 + index as i32 * 10;
        let message = prior + 1;
        source(pool, user, prior).await;
        source(pool, user, message).await;
        query("update telegram_messages set text='Всем привет' where chat_id=$1 and message_id=$2")
            .bind(CHAT)
            .bind(prior)
            .execute(pool)
            .await
            .unwrap();
        query("update telegram_messages set text=$3 where chat_id=$1 and message_id=$2")
            .bind(CHAT)
            .bind(message)
            .bind(text)
            .execute(pool)
            .await
            .unwrap();
        query("update telegram_chat_users set message_count=$3,first_seen_at=now()-make_interval(secs=>$4::double precision) where chat_id=$1 and telegram_user_id=$2")
            .bind(CHAT).bind(user).bind(if veto=="count" {11_i64} else {2})
            .bind(if veto=="old" {259201.0_f64} else {0.0}).execute(pool).await.unwrap();
        query("insert into telegram_user_profiles (telegram_user_id,first_name,username,is_bot,profile_photo_file_id,profile_photo_file_unique_id) values ($1,$2,'synthetic_private_name',$3,$4,$5)")
            .bind(user).bind(if name {"Катюша"} else {"Тест"}).bind(veto=="bot")
            .bind(avatar.then_some("synthetic-file")).bind(avatar.then_some("synthetic-unique"))
            .execute(pool).await.unwrap();
        if veto == "admin" {
            query("insert into telegram_chat_member_snapshots (chat_id,telegram_user_id,status,is_admin) values ($1,$2,'administrator',true)")
                .bind(CHAT).bind(user).execute(pool).await.unwrap();
        }
        if veto == "normal" {
            record_not_spam(pool, CHAT, user, "fixture", &serde_json::json!({}), None)
                .await
                .unwrap();
        }
        let mut c = config.clone();
        c.community.moderation.decision.initial_texts = 1;
        c.community.moderation.decision.avatar_enabled = avatar_enabled;
        c.community.moderation.decision.enabled = veto != "disabled";
        c.community.moderation.decision.external_processing_allowed = veto != "external";
        observe_message(pool, &c, CHAT, message).await.unwrap();
        observe_message(pool, &c, CHAT, message).await.unwrap();
        let jobs: Vec<(i32,i64,Value,Option<String>,String)> = sqlx::query_as("select message_id,user_id,input_json,avatar_file_id,action_status from moderation_decision_jobs where instance_id=$1 and chat_id=$2 and message_id=$3")
            .bind(&c.community.instance.id).bind(CHAT).bind(message).fetch_all(pool).await.unwrap();
        assert_eq!(
            jobs.len(),
            usize::from(expected),
            "case {index}: {text}, veto={veto}"
        );
        if let Some((actual_message, actual_user, input, avatar_file, action)) = jobs.first() {
            assert_eq!((*actual_message, *actual_user), (message, user));
            assert_eq!(input["author_message"], text);
            assert_eq!(
                input["profile_signals"]["feminine_first_name_pattern"],
                name
            );
            assert_eq!(avatar_file.is_some(), avatar && avatar_enabled);
            assert_eq!(action, "not_requested");
            let serialized = input.to_string();
            for private in [
                "Катюша",
                "synthetic_private_name",
                "synthetic-file",
                "synthetic-unique",
            ] {
                assert!(!serialized.contains(private), "privacy leak: case {index}");
            }
        }
    }
    let challenges: i64 =
        query_scalar("select count(*) from telegram_risk_captcha_challenges where chat_id=$1")
            .bind(CHAT)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(challenges, 0, "enqueue hint не выполняет Telegram actions");
}

/// Service update не расходует слот первого текста; последующий слабый сигнал
/// включает анализ, но обычные соседние сообщения не создают API jobs.
async fn assert_first_author_text_sampling(pool: &PgPool, config: &Config) {
    let user = USER + 10;
    source(pool, user, 100).await;
    query("update telegram_messages set text=null where chat_id=$1 and message_id=100")
        .bind(CHAT)
        .execute(pool)
        .await
        .unwrap();
    source(pool, user, 101).await;
    query("update telegram_messages set text='Всем привет' where chat_id=$1 and message_id=101")
        .bind(CHAT)
        .execute(pool)
        .await
        .unwrap();
    query(
        "update telegram_chat_users set message_count=2 where chat_id=$1 and telegram_user_id=$2",
    )
    .bind(CHAT)
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    observe_message(pool, config, CHAT, 100).await.unwrap();
    observe_message(pool, config, CHAT, 101).await.unwrap();
    observe_message(pool, config, CHAT, 101).await.unwrap();
    source(pool, user, 102).await;
    query("update telegram_messages set text='Хороший день' where chat_id=$1 and message_id=102")
        .bind(CHAT)
        .execute(pool)
        .await
        .unwrap();
    source(pool, user, 103).await;
    query("update telegram_messages set text=$2 where chat_id=$1 and message_id=103")
        .bind(CHAT)
        .bind("плачу за отзывы, 10к за 10 отзывов💵😉")
        .execute(pool)
        .await
        .unwrap();
    query(
        "update telegram_chat_users set message_count=4 where chat_id=$1 and telegram_user_id=$2",
    )
    .bind(CHAT)
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    observe_message(pool, config, CHAT, 102).await.unwrap();
    observe_message(pool, config, CHAT, 103).await.unwrap();
    let jobs: Vec<i32> = query_scalar("select message_id from moderation_decision_jobs where instance_id=$1 and chat_id=$2 and user_id=$3 order by message_id")
        .bind(&config.community.instance.id).bind(CHAT).bind(user).fetch_all(pool).await.unwrap();
    assert_eq!(jobs, vec![101, 103]);
    let candidates: Vec<(i32, bool)> = sqlx::query_as("select message_id,local_candidate from moderation_text_observations where instance_id=$1 and chat_id=$2 and user_id=$3 order by message_id")
        .bind(&config.community.instance.id).bind(CHAT).bind(user).fetch_all(pool).await.unwrap();
    assert_eq!(
        candidates,
        vec![(100, false), (101, false), (102, false), (103, true)]
    );
    let calls: i32 =
        query_scalar("select calls from moderation_decision_budget where instance_id=$1")
            .bind(&config.community.instance.id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        calls, 30,
        "enqueue не расходует API-квоту и не выполняет санкции"
    );
}

/// Числовая вероятность и дата доходят до модели; raw ID/имя остаются локальными.
async fn assert_temporal_id_context_snapshot(pool: &PgPool, config: &Config) {
    let user = 1_000_000_000;
    source(pool, user, 3500).await;
    query("insert into telegram_user_profiles (telegram_user_id,first_name) values ($1,'Катюша')")
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    let mut c = config.clone();
    let key = c.community.moderation.risk_profile.clone();
    let profile = c.community.risk_profiles.get_mut(&key).unwrap();
    let today = chrono::Utc::now().date_naive();
    profile.telegram_id = Some(
        serde_json::from_value(serde_json::json!({
        "floor":0.0,"ceil":1.0,"k":2.0,"midpoint_billion":0.9,"version":"synthetic-id"}))
        .unwrap(),
    );
    profile.telegram_id_time=Some(serde_json::from_value(serde_json::json!({
        "reference_date":today-chrono::Days::new(100),"midpoint_billion_per_day":0.001,"version":"synthetic-time"})).unwrap());
    observe_message(pool, &c, CHAT, 3500).await.unwrap();
    let (id,input):(i64,Value)=sqlx::query_as("select id,input_json from moderation_decision_jobs where instance_id=$1 and chat_id=$2 and message_id=3500")
        .bind(&c.community.instance.id).bind(CHAT).fetch_one(pool).await.unwrap();
    assert!(
        (input["profile_signals"]["telegram_id_spam_probability"]
            .as_f64()
            .unwrap()
            - 0.5)
            .abs()
            < 1e-12
    );
    assert_eq!(
        input["profile_signals"]["evaluated_on_utc"],
        today.to_string()
    );
    assert_eq!(
        input["profile_signals"]["feminine_first_name_pattern"],
        true
    );
    assert_eq!(input["profile_signals"]["observed_message_count"], 1);
    for private in [user.to_string(), "Катюша".into()] {
        assert!(!input.to_string().contains(&private));
    }
    c.community
        .risk_profiles
        .get_mut(&key)
        .unwrap()
        .telegram_id_time
        .as_mut()
        .unwrap()
        .midpoint_billion_per_day = 0.002;
    assert!(repo::claim(pool, &c).await.unwrap().is_none());
    let state: (String, Option<String>) =
        sqlx::query_as("select status,error_kind from moderation_decision_jobs where id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(state, ("stale".into(), Some("obsolete_version".into())));
}

async fn assert_post_topic_provenance(pool: &PgPool, config: &Config) {
    let channel = -1_009_913_000;
    let user = USER + 300;
    source(pool, user, 4002).await;
    query("insert into telegram_messages (chat_id,message_id,source_channel_id,is_automatic_forward,text) values ($1,4000,$2,true,'Rust ownership https://example.invalid/private')")
        .bind(CHAT).bind(channel).execute(pool).await.unwrap();
    query("insert into telegram_messages (chat_id,message_id,source_channel_id,is_automatic_forward,text) values ($1,4000,$2,true,'PRIVATE_NEIGHBOR')")
        .bind(CHAT-1).bind(channel).execute(pool).await.unwrap();
    query("insert into telegram_messages (chat_id,message_id,user_id,reply_to_message_id,text) values ($1,4001,$2,4000,'ORDINARY_PARENT_REPLY')")
        .bind(CHAT).bind(user+1).execute(pool).await.unwrap();
    query("update telegram_messages set reply_to_message_id=4001 where chat_id=$1 and message_id=4002")
        .bind(CHAT).execute(pool).await.unwrap();
    let mut c = config.clone();
    c.discussion_chat_id = CHAT;
    c.source_channel_id = channel;
    c.community.first_comment.routes.clear();
    observe_message(pool, &c, CHAT, 4002).await.unwrap();
    let job:repo::DecisionJob=sqlx::query_as("select * from moderation_decision_jobs where instance_id=$1 and chat_id=$2 and message_id=4002")
        .bind(&c.community.instance.id).bind(CHAT).fetch_one(pool).await.unwrap();
    assert_eq!(
        job.input_json["topic_context"]["post_topic"],
        "Rust ownership [link]"
    );
    for forbidden in [
        "PRIVATE_NEIGHBOR",
        "ORDINARY_PARENT_REPLY",
        "example.invalid",
    ] {
        assert!(!job.input_json.to_string().contains(forbidden));
    }
    assert!(repo::source_is_current(pool, &job).await.unwrap());
    c.community.moderation.decision.post_topics.push(
        serde_json::from_value(serde_json::json!({
        "discussion_chat_id":CHAT,"post_message_id":4000,"topic":"Ручная тема: Rust borrowing"}))
        .unwrap(),
    );
    observe_message(pool, &c, CHAT, 4002).await.unwrap();
    let overridden:Value=query_scalar("select input_json from moderation_decision_jobs where instance_id=$1 and chat_id=$2 and message_id=4002 order by id desc limit 1")
        .bind(&c.community.instance.id).bind(CHAT).fetch_one(pool).await.unwrap();
    assert_eq!(
        overridden["topic_context"]["post_topic"],
        "Ручная тема: Rust borrowing"
    );
    query("update telegram_messages set text='Другая тема' where chat_id=$1 and message_id=4000")
        .bind(CHAT)
        .execute(pool)
        .await
        .unwrap();
    assert!(!repo::source_is_current(pool, &job).await.unwrap());
    query("delete from telegram_messages where chat_id=$1 and message_id=4000")
        .bind(CHAT)
        .execute(pool)
        .await
        .unwrap();
    observe_message(pool, &c, CHAT, 4002).await.unwrap();
    let newest:Value=query_scalar("select input_json from moderation_decision_jobs where instance_id=$1 and chat_id=$2 and message_id=4002 order by id desc limit 1")
        .bind(&c.community.instance.id).bind(CHAT).fetch_one(pool).await.unwrap();
    assert!(newest["topic_context"]["post_topic"].is_null());
}
