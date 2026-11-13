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
