use std::{
    collections::HashSet,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use sqlx::PgPool;
use teloxide::prelude::*;
use tokio::sync::mpsc;

use crate::{
    config::Config,
    db::telegram::{mark_user_profile_refresh_error, user_profile_needs_refresh},
    features::jobs::worker::{WorkerId, WorkerSupervisor},
    features::user_profiles::service::refresh_profile,
};

#[cfg(feature = "moderation")]
use crate::features::jobs::{
    policy::PROFILE_REFRESH_INTENT_POLL,
    worker::{PollOutcome, WorkerSpec, run_poll_worker},
};

#[cfg(feature = "moderation")]
use crate::features::{
    new_user_analysis::enqueue_new_user_audit_for_profile_refresh_with_url_feed,
    url_feeds::FeedSnapshot,
};

const PROFILE_REFRESH_QUEUE_CAPACITY: usize = 256;
#[cfg(feature = "moderation")]
const PROFILE_REFRESH_INTENT_BATCH: i64 = 64;
#[cfg(feature = "moderation")]
const PROFILE_REFRESH_INTENT_MAX_ATTEMPTS: i32 = 8;

/// Profile snapshots are global per Telegram user while refresh jobs are keyed
/// by chat. Two chats can therefore race one upstream fetch. This process-wide
/// per-user gate admits at most one fetch at a time; the losing worker still
/// runs its own per-chat audit.
const PROFILE_REFRESH_STRIPES: usize = 256;
static PROFILE_REFRESH_GATES: LazyLock<[tokio::sync::Mutex<()>; PROFILE_REFRESH_STRIPES]> =
    LazyLock::new(|| std::array::from_fn(|_| tokio::sync::Mutex::new(())));

fn profile_refresh_gate(user_id: i64) -> &'static tokio::sync::Mutex<()> {
    &PROFILE_REFRESH_GATES[user_id as usize % PROFILE_REFRESH_STRIPES]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileRefreshEnqueueResult {
    Queued,
    Coalesced,
    Full,
    Closed,
}

#[derive(Clone)]
pub struct ProfileRefreshQueue {
    sender: mpsc::Sender<ProfileRefreshJob>,
    queued_jobs: Arc<Mutex<HashSet<(i64, i64)>>>,
    full_events: Arc<AtomicU64>,
}

#[derive(Clone, Copy)]
struct ProfileRefreshJob {
    chat_id: i64,
    user_id: i64,
}

impl ProfileRefreshQueue {
    fn new(
        capacity: usize,
    ) -> (
        Self,
        Arc<tokio::sync::Mutex<mpsc::Receiver<ProfileRefreshJob>>>,
    ) {
        let (sender, receiver) = mpsc::channel(capacity);
        let queue = Self {
            sender,
            queued_jobs: Arc::new(Mutex::new(HashSet::new())),
            full_events: Arc::new(AtomicU64::new(0)),
        };
        (queue, Arc::new(tokio::sync::Mutex::new(receiver)))
    }

    pub fn try_enqueue(&self, chat_id: i64, user_id: i64) -> ProfileRefreshEnqueueResult {
        let mut queued_jobs = self
            .queued_jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = (chat_id, user_id);
        if !queued_jobs.insert(key) {
            return ProfileRefreshEnqueueResult::Coalesced;
        }

        let job = ProfileRefreshJob { chat_id, user_id };
        match self.sender.try_send(job) {
            Ok(()) => ProfileRefreshEnqueueResult::Queued,
            Err(mpsc::error::TrySendError::Full(job)) => {
                queued_jobs.remove(&(job.chat_id, job.user_id));
                self.full_events.fetch_add(1, Ordering::Relaxed);
                ProfileRefreshEnqueueResult::Full
            }
            Err(mpsc::error::TrySendError::Closed(job)) => {
                queued_jobs.remove(&(job.chat_id, job.user_id));
                ProfileRefreshEnqueueResult::Closed
            }
        }
    }

    fn mark_completed(&self, job: ProfileRefreshJob) {
        self.queued_jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&(job.chat_id, job.user_id));
    }

    /// Number of events rejected by the bounded accelerator. The durable
    /// intent table is still authoritative and reports recovery separately.
    pub fn full_events(&self) -> u64 {
        self.full_events.load(Ordering::Relaxed)
    }
}

pub fn spawn_profile_refresh_workers(
    supervisor: &mut WorkerSupervisor,
    bot: Bot,
    pool: PgPool,
    config: Arc<Config>,
    #[cfg(feature = "moderation")] url_feed_snapshot: Arc<
        tokio::sync::RwLock<Option<Arc<FeedSnapshot>>>,
    >,
) -> ProfileRefreshQueue {
    let (queue, receiver) = ProfileRefreshQueue::new(PROFILE_REFRESH_QUEUE_CAPACITY);

    for worker_index in 0..config.profile_refresh_concurrency {
        let bot = bot.clone();
        let pool = pool.clone();
        let config = Arc::clone(&config);
        #[cfg(feature = "moderation")]
        let url_feed_snapshot = url_feed_snapshot.clone();
        let queue = queue.clone();
        let receiver = receiver.clone();
        supervisor.spawn(WorkerId::ProfileRefresh, move |mut shutdown| async move {
            let mut draining = false;
            while let Some(job) =
                next_profile_refresh_job(&receiver, &mut shutdown, &mut draining).await
            {
                process_profile_refresh_job(
                    &bot,
                    &pool,
                    &config,
                    #[cfg(feature = "moderation")]
                    url_feed_snapshot.clone(),
                    job,
                )
                .await;
                queue.mark_completed(job);
            }
            tracing::warn!(worker_index, "profile refresh queue worker stopped");
        });
    }

    queue
}

#[cfg(feature = "moderation")]
pub fn spawn_profile_refresh_intent_reconciler(
    supervisor: &mut WorkerSupervisor,
    pool: PgPool,
    config: Arc<Config>,
    url_feed_snapshot: Arc<tokio::sync::RwLock<Option<Arc<FeedSnapshot>>>>,
) {
    supervisor.spawn(
        WorkerId::ProfileRefreshReconciler,
        move |shutdown| async move {
            run_poll_worker(
                WorkerSpec::new(
                    WorkerId::ProfileRefreshReconciler,
                    PROFILE_REFRESH_INTENT_POLL,
                ),
                shutdown,
                || {
                    let pool = pool.clone();
                    let config = Arc::clone(&config);
                    let url_feed_snapshot = Arc::clone(&url_feed_snapshot);
                    async move {
                        let url_feed = url_feed_snapshot.read().await.clone();
                        match reconcile_profile_refresh_intents(
                            &pool,
                            &config,
                            url_feed,
                            PROFILE_REFRESH_INTENT_BATCH,
                        )
                        .await
                        {
                            Ok(0) => PollOutcome::Idle,
                            Ok(_) => PollOutcome::Processed,
                            Err(_) => PollOutcome::Failed("intent_reconcile_failed"),
                        }
                    }
                },
            )
            .await;
        },
    );
}

async fn next_profile_refresh_job(
    receiver: &Arc<tokio::sync::Mutex<mpsc::Receiver<ProfileRefreshJob>>>,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    draining: &mut bool,
) -> Option<ProfileRefreshJob> {
    loop {
        if *draining {
            return receiver.lock().await.recv().await;
        }
        tokio::select! {
            job = async { receiver.lock().await.recv().await } => return job,
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    receiver.lock().await.close();
                    *draining = true;
                }
            }
        }
    }
}

async fn process_profile_refresh_job(
    bot: &Bot,
    pool: &PgPool,
    config: &Config,
    #[cfg(feature = "moderation")] url_feed_snapshot: Arc<
        tokio::sync::RwLock<Option<Arc<FeedSnapshot>>>,
    >,
    job: ProfileRefreshJob,
) {
    // A second chat for the same user must not start a concurrent upstream
    // fetch. The winner re-checks freshness after taking the gate, so a queue
    // backlog that became stale while waiting performs no extra API call.
    {
        let _gate = profile_refresh_gate(job.user_id).lock().await;
        let should_refresh = match user_profile_needs_refresh(pool, job.user_id).await {
            Ok(should_refresh) => should_refresh,
            Err(err) => {
                tracing::warn!(%err, user_id = job.user_id, "failed to check user profile refresh state");
                false
            }
        };

        if should_refresh {
            match refresh_profile(bot, pool, job.user_id).await {
                Ok(()) => {
                    if let Err(err) = crate::features::spammer_avatar_embeddings::enqueue_refreshed_spammer_avatars(
                        pool,
                        config.avatar_embeddings_enabled,
                        job.user_id,
                    )
                    .await
                    {
                        tracing::warn!(%err, user_id = job.user_id, "failed to queue confirmed spammer avatar");
                    }
                }
                Err(err) => {
                    let message = err.to_string();
                    if let Err(save_err) =
                        mark_user_profile_refresh_error(pool, job.user_id, &message).await
                    {
                        tracing::warn!(%save_err, user_id = job.user_id, "failed to save profile refresh error");
                    }
                    tracing::warn!(%err, user_id = job.user_id, "failed to refresh message author profile");
                }
            }
        }
    }

    // Profile freshness is global, while the audit is keyed by (chat, user).
    // A fresh profile must not suppress the first audit in another community chat.
    process_profile_audit(
        pool,
        config,
        #[cfg(feature = "moderation")]
        url_feed_snapshot,
        job,
    )
    .await;
}

async fn process_profile_audit(
    pool: &PgPool,
    config: &Config,
    #[cfg(feature = "moderation")] url_feed_snapshot: Arc<
        tokio::sync::RwLock<Option<Arc<FeedSnapshot>>>,
    >,
    job: ProfileRefreshJob,
) {
    #[cfg(not(feature = "moderation"))]
    let _ = (pool, config, job);

    #[cfg(feature = "moderation")]
    if crate::features::decision_audit::service::observe_latest_author_message(
        pool,
        config,
        job.chat_id,
        job.user_id,
    )
    .await
    .is_err()
    {
        tracing::warn!(
            user_id = job.user_id,
            "decision shadow avatar refresh observation failed"
        );
    }

    #[cfg(feature = "moderation")]
    if config.new_user_audit_enabled
        && config.chat_allows(job.chat_id, |chat| chat.moderation)
        && let Err(err) = enqueue_new_user_audit_for_profile_refresh_with_url_feed(
            pool,
            config,
            job.chat_id,
            job.user_id,
            url_feed_snapshot.read().await.clone(),
        )
        .await
    {
        tracing::warn!(%err, user_id = job.user_id, "failed to save unified new user audit baseline and job");
    }
}

/// Replays durable ingest intents that the bounded queue could not accept or
/// that were interrupted by a crash. Each stored revision is completed at most
/// once; a newer message increments the revision and reopens the intent.
#[cfg(feature = "moderation")]
pub async fn reconcile_profile_refresh_intents(
    pool: &PgPool,
    config: &Config,
    url_feed: Option<Arc<FeedSnapshot>>,
    limit: i64,
) -> anyhow::Result<usize> {
    if !config.new_user_audit_enabled || limit <= 0 {
        return Ok(0);
    }
    let instance_id = &config.community.instance.id;
    let chat_ids: Vec<i64> = config
        .community
        .chats
        .values()
        .filter(|chat| chat.ingest && chat.moderation)
        .map(|chat| chat.id)
        .collect();
    let mut tx = pool.begin().await?;
    let rows: Vec<(i64, i64, i64, i32, i64)> = sqlx::query_as(
        r#"
        select chat_id, telegram_user_id, revision, attempts,
               greatest(0, extract(epoch from now() - created_at))::bigint
        from public.telegram_profile_refresh_intents
        where instance_id = $1 and chat_id = any($3)
          and (completed_revision is null or completed_revision < revision)
          and failed_at is null
          and next_attempt_at <= now()
        order by next_attempt_at, chat_id, telegram_user_id
        for update skip locked
        limit $2
        "#,
    )
    .bind(instance_id)
    .bind(limit)
    .bind(&chat_ids)
    .fetch_all(&mut *tx)
    .await?;
    for (chat_id, user_id, revision, attempts, _) in &rows {
        sqlx::query(
            r#"
            update public.telegram_profile_refresh_intents
            set attempts = case when $5 then attempts else attempts + 1 end,
                failed_at = case when $5 then now() else null end,
                last_error = case when $5 then 'lease_exhausted' else last_error end,
                next_attempt_at = now() + interval '2 minutes',
                updated_at = now()
            where instance_id = $1 and chat_id = $2 and telegram_user_id = $3
              and revision = $4
            "#,
        )
        .bind(instance_id)
        .bind(chat_id)
        .bind(user_id)
        .bind(revision)
        .bind(*attempts >= PROFILE_REFRESH_INTENT_MAX_ATTEMPTS)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    let mut recovered = 0usize;
    for (chat_id, user_id, revision, attempts, pending_age_seconds) in rows {
        if attempts >= PROFILE_REFRESH_INTENT_MAX_ATTEMPTS {
            continue;
        }
        match enqueue_new_user_audit_for_profile_refresh_with_url_feed(
            pool,
            config,
            chat_id,
            user_id,
            url_feed.clone(),
        )
        .await
        {
            Ok(()) => {
                let result = sqlx::query(
                    r#"
                    update public.telegram_profile_refresh_intents
                    set completed_revision = $4,
                        failed_at = null,
                        last_error = null,
                        updated_at = now()
                    where instance_id = $1 and chat_id = $2 and telegram_user_id = $3
                      and revision = $4 and attempts = $5
                      and failed_at is null and completed_revision is distinct from $4
                    "#,
                )
                .bind(instance_id)
                .bind(chat_id)
                .bind(user_id)
                .bind(revision)
                .bind(attempts + 1)
                .execute(pool)
                .await?;
                recovered += result.rows_affected() as usize;
                tracing::info!(
                    chat_id,
                    user_id,
                    revision,
                    attempts = attempts + 1,
                    pending_age_seconds,
                    recovery_count = recovered,
                    "recovered durable profile refresh intent"
                );
            }
            Err(error) => {
                let terminal = attempts + 1 >= PROFILE_REFRESH_INTENT_MAX_ATTEMPTS;
                let delay_seconds = 15_i64
                    .saturating_mul(1_i64 << (attempts.clamp(0, 10) as u32))
                    .min(3_600);
                sqlx::query(
                    r#"
                    update public.telegram_profile_refresh_intents
                    set next_attempt_at = case
                            when $5 then now() + interval '1 day'
                            else now() + make_interval(secs => $6)
                        end,
                        failed_at = case when $5 then now() else failed_at end,
                        last_error = 'audit_materialization_failed',
                        updated_at = now()
                    where instance_id = $1 and chat_id = $2 and telegram_user_id = $3
                      and revision = $4 and attempts = $7
                      and failed_at is null and completed_revision is distinct from $4
                    "#,
                )
                .bind(instance_id)
                .bind(chat_id)
                .bind(user_id)
                .bind(revision)
                .bind(terminal)
                .bind(delay_seconds)
                .bind(attempts + 1)
                .execute(pool)
                .await?;
                tracing::warn!(
                    %error,
                    chat_id,
                    user_id,
                    revision,
                    attempts = attempts + 1,
                    terminal,
                    "durable profile refresh intent failed"
                );
            }
        }
    }
    Ok(recovered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "moderation")]
    #[tokio::test]
    #[ignore = "requires a disposable migrated PostgreSQL database"]
    async fn two_chats_share_profile_fetch_but_keep_both_audits() {
        use axum::{Json, Router, http::Uri, routing::post};
        use serde_json::json;
        use tokio::sync::{Notify, RwLock};

        let pool = PgPool::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let suffix = uuid::Uuid::new_v4().as_u128() as u32;
        let user = 8_992_000_000_000 + i64::from(suffix);
        let chat = -1_008_992_000_000 - 2 * i64::from(suffix);
        let mut cfg = crate::config::test_config();
        cfg.new_user_audit_enabled = true;
        cfg.community.instance.id = format!("singleflight_{suffix}");
        let main = cfg.community.chats.get_mut("main").unwrap();
        main.id = chat;
        let mut other = main.clone();
        other.id = chat - 1;
        cfg.community.chats.insert("other".into(), other);
        cfg.chat_registry = crate::community::ChatRegistry::new(&cfg.community).unwrap();
        for chat in [chat, chat - 1] {
            let msg = serde_json::from_value(json!({
                "message_id":1, "date":chrono::Utc::now().timestamp(),
                "chat":{"id":chat,"type":"supergroup","title":"Fixture"},
                "from":{"id":user,"is_bot":false,"first_name":"Fixture"},"text":"Fixture"
            }))
            .unwrap();
            assert!(
                crate::features::ingest::ingest_message(&pool, &msg, &cfg)
                    .await
                    .unwrap()
                    .was_persisted()
            );
        }

        let arrived = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicU64::new(0));
        let fake = {
            let (arrived, release, calls) = (arrived.clone(), release.clone(), calls.clone());
            move |uri: Uri| {
                let (arrived, release, calls) = (arrived.clone(), release.clone(), calls.clone());
                async move {
                    let result = match uri
                        .path()
                        .rsplit('/')
                        .next()
                        .unwrap()
                        .to_ascii_lowercase()
                        .as_str()
                    {
                        "getchat" => {
                            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                                arrived.notify_one();
                                release.notified().await;
                            }
                            json!({"id":user,"type":"private","first_name":"Fixture",
                                "username":"fixture_current","max_reaction_count":0,
                                "accepted_gift_types":{"unlimited_gifts":false,"limited_gifts":false,
                                     "unique_gifts":false,"premium_subscription":false}})
                        }
                        "getuserprofilephotos" => json!({"total_count":0,"photos":[]}),
                        "getuserpersonalchatmessages" => json!([]),
                        other => panic!("unexpected method {other}"),
                    };
                    Json(json!({"ok":true,"result":result}))
                }
            }
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().fallback(post(fake)))
                .await
                .unwrap()
        });
        let bot = Bot::new("fixture").set_api_url(format!("http://{address}/").parse().unwrap());
        let feed = Arc::new(RwLock::new(None));
        let run = |chat_id| {
            process_profile_refresh_job(
                &bot,
                &pool,
                &cfg,
                feed.clone(),
                ProfileRefreshJob {
                    chat_id,
                    user_id: user,
                },
            )
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let first = run(chat);
            tokio::pin!(first);
            tokio::select! {
                _ = &mut first => panic!("refresh returned before upstream was released"),
                _ = arrived.notified() => {}
            }
            assert!(
                profile_refresh_gate(user).try_lock().is_err(),
                "fetch must own the gate"
            );
            let second = run(chat - 1);
            tokio::pin!(second);
            // Poll the second job while the first still owns the gate.
            tokio::select! {
                biased;
                _ = &mut second => panic!("second job bypassed the active refresh"),
                _ = std::future::ready(()) => {}
            }
            release.notify_one();
            tokio::join!(first, second);
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let alias: String = sqlx::query_scalar(
            "select username_normalized from telegram_username_aliases where telegram_peer_id=$1 and is_active",
        ).bind(user).fetch_one(&pool).await.unwrap();
        assert_eq!(alias, "fixture_current");
        let chats: Vec<i64> = sqlx::query_scalar("select chat_id from telegram_new_user_profile_audits where telegram_user_id=$1 order by chat_id")
            .bind(user).fetch_all(&pool).await.unwrap();
        assert_eq!(chats, [chat - 1, chat]);
        server.abort();
    }

    #[cfg(feature = "moderation")]
    #[tokio::test]
    #[ignore = "requires a disposable migrated PostgreSQL database"]
    async fn saturated_and_closed_queue_preserves_ingested_audit_intents() {
        let pool = PgPool::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let mut config = crate::config::test_config();
        let fixture = uuid::Uuid::new_v4().as_u128() as u32;
        let chat_id = -1_008_000_000_000 - i64::from(fixture);
        let user_base = 8_000_000_000_000 + i64::from(fixture) * 512;
        config.community.instance.id = format!("queue_fixture_{fixture}");
        config.community.chats.get_mut("main").unwrap().id = chat_id;
        config.chat_registry = crate::community::ChatRegistry::new(&config.community).unwrap();
        config.new_user_audit_enabled = true;

        let (queue, receiver) = ProfileRefreshQueue::new(PROFILE_REFRESH_QUEUE_CAPACITY);
        let total = PROFILE_REFRESH_QUEUE_CAPACITY + 3;
        for index in 0..total {
            let user_id = user_base + index as i64;
            let msg: Message = serde_json::from_value(serde_json::json!({
                "message_id": index + 1,
                "date": chrono::Utc::now().timestamp(),
                "chat": {"id": chat_id, "type": "supergroup", "title": "Queue fixture"},
                "from": {"id": user_id, "is_bot": false, "first_name": "Fixture"},
                "text": "Synthetic audit fixture"
            }))
            .unwrap();
            let ingest = || crate::features::ingest::ingest_message(&pool, &msg, &config);
            assert!(ingest().await.unwrap().was_persisted());
            if index == total - 1 {
                // Commit before a simulated process exit: no queue send at all.
                assert!(ingest().await.unwrap().was_persisted());
                break;
            }
            let expected = match index {
                PROFILE_REFRESH_QUEUE_CAPACITY => ProfileRefreshEnqueueResult::Full,
                index if index > PROFILE_REFRESH_QUEUE_CAPACITY => {
                    receiver.lock().await.close();
                    ProfileRefreshEnqueueResult::Closed
                }
                _ => ProfileRefreshEnqueueResult::Queued,
            };
            assert_eq!(queue.try_enqueue(chat_id, user_id), expected);
        }
        assert_eq!(queue.full_events(), 1);
        drop((queue, receiver));

        let mut recovered = 0;
        loop {
            let count = reconcile_profile_refresh_intents(&pool, &config, None, 32)
                .await
                .unwrap();
            if count == 0 {
                break;
            }
            recovered += count;
            assert!(recovered <= total);
        }
        assert_eq!(recovered, total);
        let (intents, baselines): (i64, i64) = sqlx::query_as(
            r#"select
                (select count(*) from telegram_profile_refresh_intents
                  where instance_id=$1 and chat_id=$2 and revision=1 and completed_revision=1),
                (select count(*) from telegram_new_user_profile_audits where chat_id=$2)"#,
        )
        .bind(&config.community.instance.id)
        .bind(chat_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((intents, baselines), (total as i64, total as i64));
    }

    #[tokio::test]
    async fn queue_coalesces_user_events_and_releases_them_after_completion() {
        let (queue, receiver) = ProfileRefreshQueue::new(1);

        assert_eq!(
            queue.try_enqueue(-1001, 42),
            ProfileRefreshEnqueueResult::Queued
        );
        assert_eq!(
            queue.try_enqueue(-1001, 42),
            ProfileRefreshEnqueueResult::Coalesced
        );
        assert_eq!(
            queue.try_enqueue(-1002, 42),
            ProfileRefreshEnqueueResult::Full
        );
        assert_eq!(
            queue.try_enqueue(-1001, 43),
            ProfileRefreshEnqueueResult::Full
        );

        let job = receiver.lock().await.recv().await.unwrap();
        assert_eq!(job.user_id, 42);
        queue.mark_completed(job);

        assert_eq!(
            queue.try_enqueue(-1001, 42),
            ProfileRefreshEnqueueResult::Queued
        );
    }

    #[tokio::test]
    async fn shutdown_closes_intake_then_drains_already_queued_profiles() {
        let (empty_queue, empty_receiver) = ProfileRefreshQueue::new(1);
        let (shutdown_tx, mut shutdown) = tokio::sync::watch::channel(false);
        let mut draining = false;
        shutdown_tx.send_replace(true);
        assert!(
            next_profile_refresh_job(&empty_receiver, &mut shutdown, &mut draining)
                .await
                .is_none()
        );
        assert!(draining);
        assert_eq!(
            empty_queue.try_enqueue(-1001, 41),
            ProfileRefreshEnqueueResult::Closed
        );

        let (queue, receiver) = ProfileRefreshQueue::new(2);
        assert_eq!(
            queue.try_enqueue(-1001, 42),
            ProfileRefreshEnqueueResult::Queued
        );
        receiver.lock().await.close();
        let (_shutdown_tx, mut shutdown) = tokio::sync::watch::channel(true);
        let mut draining = true;

        let queued = next_profile_refresh_job(&receiver, &mut shutdown, &mut draining)
            .await
            .expect("shutdown should retain already queued work");
        assert_eq!(queued.user_id, 42);
        assert_eq!(
            queue.try_enqueue(-1001, 43),
            ProfileRefreshEnqueueResult::Closed
        );

        queue.mark_completed(queued);
        assert!(
            next_profile_refresh_job(&receiver, &mut shutdown, &mut draining)
                .await
                .is_none()
        );
    }
}
