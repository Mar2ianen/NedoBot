use std::{
    collections::HashSet,
    sync::{Arc, LazyLock},
    time::Duration,
};

use teloxide::{net::Download, prelude::*, types::FileId};

use crate::{
    db::url_evidence::{
        MAX_QR_URLS_PER_MESSAGE, QrScanJob, QrUrlEvidence, UrlShadowRefreshJob,
        claim_next_qr_scan_job, claim_next_url_shadow_refresh_job, finish_qr_scan_job,
        finish_url_shadow_refresh_job, replace_qr_evidence_for_job,
    },
    features::jobs::{
        policy::WorkerPollPolicy,
        worker::{PollOutcome, WorkerId, WorkerSpec, WorkerSupervisor, run_poll_worker},
    },
    state::AppState,
    telegram::media::LimitedBytesWriter,
};

const QR_SCAN_TIMEOUT: Duration = Duration::from_secs(120);
static QR_SCAN_CPU_SLOT: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(1)));

pub fn spawn_worker(supervisor: &mut WorkerSupervisor, bot: Bot, state: AppState) {
    let shadow_state = state.clone();
    supervisor.spawn(WorkerId::QrEvidence, move |shutdown| async move {
        run_poll_worker(
            WorkerSpec::new(WorkerId::QrEvidence, WorkerPollPolicy::new(3, 3)),
            shutdown,
            || async {
                match process_next_job(&bot, &state).await {
                    Ok(true) => PollOutcome::Processed,
                    Ok(false) => PollOutcome::Idle,
                    Err(error_kind) => PollOutcome::Failed(error_kind),
                }
            },
        )
        .await;
    });
    supervisor.spawn(WorkerId::QrShadowRefresh, move |shutdown| async move {
        run_poll_worker(
            WorkerSpec::new(WorkerId::QrShadowRefresh, WorkerPollPolicy::new(3, 3)),
            shutdown,
            || async {
                match process_next_shadow_refresh(&shadow_state).await {
                    Ok(true) => PollOutcome::Processed,
                    Ok(false) => PollOutcome::Idle,
                    Err(error_kind) => PollOutcome::Failed(error_kind),
                }
            },
        )
        .await;
    });
}

async fn process_next_job(bot: &Bot, state: &AppState) -> Result<bool, &'static str> {
    let job = claim_next_qr_scan_job(&state.pool)
        .await
        .map_err(|_| "claim_failed")?;
    let Some(job) = job else {
        return Ok(false);
    };

    let processing = tokio::time::timeout(QR_SCAN_TIMEOUT, process_job(bot, state, &job)).await;
    match processing {
        Ok(Ok(Some(reference_count))) => {
            tracing::debug!(
                job_id = job.id,
                reference_count,
                "QR message evidence committed with downstream shadow refresh queued"
            );
        }
        Ok(Ok(None)) => {
            tracing::debug!(
                job_id = job.id,
                "QR scan result discarded after message or lease changed"
            );
        }
        Ok(Err(error_kind)) => {
            finish_qr_scan_job(&state.pool, &job, None, Some(error_kind))
                .await
                .map_err(|_| "finish_failed")?;
            tracing::warn!(
                job_id = job.id,
                error_kind,
                "QR message evidence scan failed"
            );
        }
        Err(_) => {
            finish_qr_scan_job(&state.pool, &job, None, Some("processing_timeout"))
                .await
                .map_err(|_| "finish_failed")?;
            tracing::warn!(job_id = job.id, "QR message evidence scan timed out");
        }
    }
    Ok(true)
}

async fn process_job(
    bot: &Bot,
    state: &AppState,
    job: &QrScanJob,
) -> Result<Option<usize>, &'static str> {
    let pool = &state.pool;
    let file = bot
        .get_file(FileId(job.file_id.clone()))
        .await
        .map_err(|_| "telegram_api")?;
    if usize::try_from(file.size).unwrap_or(usize::MAX) > nedobot_qr::MAX_COMPRESSED_BYTES {
        return Err("file_too_large");
    }

    let mut writer = LimitedBytesWriter::new(nedobot_qr::MAX_COMPRESSED_BYTES);
    bot.download_file(&file.path, &mut writer)
        .await
        .map_err(|_| "download_failed")?;
    let bytes = writer.into_inner();
    let payloads = run_bounded_qr_decode(bytes).await?;

    let urls = qr_urls_from_payloads(&payloads);

    let affiliated =
        crate::features::url_relations::load_affiliated_peer_ids(pool, &state.config, job.chat_id)
            .await
            .map_err(|_| "affiliated_channels_lookup_failed")?;
    let mut classified_urls = Vec::with_capacity(urls.len());
    for target in urls {
        // Persist the same fresh unique-ownership decision used elsewhere for
        // Telegram username links. The pure classifier would silently turn
        // every `t.me/<name>/...` link into Unresolved.
        let relation = crate::features::url_relations::classify_qr_url_with_aliases(
            pool,
            &target,
            job.chat_id,
            &affiliated,
        )
        .await
        .map_err(|_| "alias_resolution_failed")?;
        classified_urls.push(QrUrlEvidence { relation, target });
    }
    let materialized = replace_qr_evidence_for_job(
        pool,
        &state.config.community.instance.id,
        job,
        &classified_urls,
    )
    .await
    .map_err(|_| "evidence_write_failed")?;
    Ok(materialized)
}

async fn run_bounded_qr_decode(bytes: Vec<u8>) -> Result<Vec<Vec<u8>>, &'static str> {
    run_bounded_qr_decode_with(Arc::clone(&QR_SCAN_CPU_SLOT), move || {
        nedobot_qr::scan_image(&bytes)
    })
    .await
    .map_err(|_| "decoder_join_failed")?
    .map_err(|_| "image_decode_failed")
}

async fn run_bounded_qr_decode_with<F, R>(
    cpu_slot: Arc<tokio::sync::Semaphore>,
    decode: F,
) -> Result<R, tokio::task::JoinError>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let permit = cpu_slot
        .acquire_owned()
        .await
        .expect("QR decoder CPU semaphore is never closed");
    tokio::task::spawn_blocking(move || {
        // A timed-out JoinHandle detaches this blocking task. Keep the permit
        // inside the closure so a later job cannot create unbounded CPU work.
        let _permit = permit;
        decode()
    })
    .await
}

async fn process_next_shadow_refresh(state: &AppState) -> Result<bool, &'static str> {
    let job = claim_next_url_shadow_refresh_job(&state.pool, &state.config.community.instance.id)
        .await
        .map_err(|_| "shadow_claim_failed")?;
    let Some(job) = job else {
        return Ok(false);
    };
    let url_feed = state.url_feed_snapshot.read().await.clone();
    refresh_shadow_for_job(state, &job, url_feed.as_deref()).await
}

async fn refresh_shadow_for_job(
    state: &AppState,
    job: &UrlShadowRefreshJob,
    url_feed: Option<&crate::features::url_feeds::FeedSnapshot>,
) -> Result<bool, &'static str> {
    let result = crate::features::new_user_analysis::refresh_url_shadow_observation_from_links(
        &state.pool,
        &state.config,
        job.chat_id,
        job.telegram_user_id,
        url_feed,
    )
    .await;
    match result {
        Ok(()) => {
            finish_url_shadow_refresh_job(&state.pool, job, None)
                .await
                .map_err(|_| "shadow_finish_failed")?;
            tracing::debug!(job_id = job.id, "URL shadow refresh completed");
        }
        Err(_) => {
            finish_url_shadow_refresh_job(&state.pool, job, Some("refresh_failed"))
                .await
                .map_err(|_| "shadow_finish_failed")?;
            tracing::warn!(
                job_id = job.id,
                attempt = job.attempts,
                "URL shadow refresh will retry"
            );
        }
    }
    Ok(true)
}

fn qr_urls_from_payloads(payloads: &[Vec<u8>]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for payload in payloads {
        let Some(text) = nedobot_qr::as_utf8(payload) else {
            continue;
        };
        for target in teloxide_url::find_urls(text) {
            if target.kind == teloxide_url::Kind::Url {
                let Some(canonical) =
                    teloxide_url::parse_url(&target.value).map(|url| url.to_string())
                else {
                    continue;
                };
                if seen.insert(canonical.clone()) {
                    urls.push(canonical);
                }
                if urls.len() >= MAX_QR_URLS_PER_MESSAGE {
                    break;
                }
            }
        }
        if urls.len() >= MAX_QR_URLS_PER_MESSAGE {
            break;
        }
    }
    urls
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use super::{qr_urls_from_payloads, run_bounded_qr_decode_with};

    #[tokio::test]
    #[ignore = "requires a disposable migrated PostgreSQL database"]
    async fn qr_worker_persists_fresh_alias_relation_and_rejects_superseded_media() {
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        use serde_json::json;
        use teloxide::prelude::*;

        const PNG: &[u8] = include_bytes!("../../tests/fixtures/deep_audit_qr.png");
        let pool = sqlx::PgPool::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let mut config = crate::config::test_config();
        let chat = config.discussion_chat_id;
        config.community.instance.id = "qr_worker_fixture".into();
        let state = crate::state::AppState::new(pool.clone(), config);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .route(
                "/botfixture/GetFile",
                post(|| async {
                    Json(
                        json!({"ok":true,"result":{"file_id":"fixture","file_unique_id":"qr-a",
                    "file_size":PNG.len(),"file_path":"fixture.png"}}),
                    )
                }),
            )
            .route("/file/botfixture/fixture.png", get(|| async { PNG }));
        let fake = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let bot = Bot::new("fixture").set_api_url(format!("http://{address}/").parse().unwrap());
        let user_id = 8_975_501_031_i64;
        let name = "fixture_qr_alias";
        let owner = -1_009_755_010_000_i64;
        let fixture_pool = &pool;
        let insert_alias = |id, age| async move {
            sqlx::query("insert into telegram_username_aliases (username_normalized,telegram_peer_id,peer_type,source,observed_at) values ($1,$2,'channel','configured_chat_refresh',now()-$3*interval '1 hour')")
                .bind(name).bind(id).bind(age).execute(fixture_pool).await.unwrap();
        };
        sqlx::query("delete from telegram_username_aliases where username_normalized=$1")
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();
        insert_alias(owner, 0_i32).await;
        for (index, expected) in [
            "external",
            "unresolved",
            "unresolved",
            "affiliated",
            "superseded",
        ]
        .into_iter()
        .enumerate()
        {
            match index {
                1 => insert_alias(chat, 0).await,
                2 => {
                    sqlx::query("update telegram_username_aliases set observed_at=now()-interval '25 hours' where username_normalized=$1")
                        .bind(name).execute(&pool).await.unwrap();
                }
                3 => {
                    sqlx::query("update telegram_username_aliases set observed_at=now() where username_normalized=$1 and telegram_peer_id=$2")
                        .bind(name).bind(chat).execute(&pool).await.unwrap();
                }
                _ => {}
            }
            let message_id = 1_712_340_000 + index as i32;
            let media = if expected == "superseded" {
                "qr-b"
            } else {
                "qr-a"
            };
            let msg = json!({
                "message_id":message_id, "date":chrono::Utc::now().timestamp(),
                "chat":{"id":chat,"type":"supergroup","title":"QR fixture"},
                "from":{"id":user_id,"is_bot":false,"first_name":"Fixture"},
                "photo":[{"file_id":"fixture","file_unique_id":media,"width":185,"height":185}]
            });
            sqlx::query("insert into telegram_messages (chat_id,message_id,user_id,raw_json) values ($1,$2,$3,$4) on conflict (chat_id,message_id) do update set raw_json=excluded.raw_json, created_at=now()")
                .bind(chat).bind(message_id).bind(user_id).bind(msg).execute(&pool).await.unwrap();
            let job = sqlx::query_as::<_, crate::db::url_evidence::QrScanJob>(
                "insert into telegram_qr_scan_jobs (chat_id,message_id,file_id,file_unique_id,status,attempts,lease_generation,lease_expires_at) values ($1,$2,'fixture','qr-a','leased',1,1,now()+interval '10 minutes') on conflict (chat_id,message_id,file_unique_id) do update set status='leased',lease_expires_at=excluded.lease_expires_at returning id,chat_id,message_id,file_id,file_unique_id,attempts,lease_generation",
            ).bind(chat).bind(message_id).fetch_one(&pool).await.unwrap();
            assert_eq!(
                super::process_job(&bot, &state, &job).await,
                Ok((expected != "superseded").then_some(1))
            );
            let relation: Option<String> = sqlx::query_scalar("select qr_relation from telegram_message_url_evidence where chat_id=$1 and message_id=$2 and source='qr'")
                .bind(chat).bind(message_id).fetch_optional(&pool).await.unwrap();
            assert_eq!(
                relation.as_deref(),
                (expected != "superseded").then_some(expected)
            );
            if index == 0 {
                let summary = crate::features::url_relations::recent_link_relationship_summary(
                    &pool,
                    &state.config,
                    chat,
                    user_id,
                    None,
                )
                .await
                .unwrap();
                assert_eq!(summary["external_qr_url_count"], 1);
            }
        }
        fake.abort();
    }

    #[test]
    fn qr_deduplication_preserves_case_sensitive_path_and_query() {
        let payloads = vec![
            b"https://example.org/File?campaign=Normal".to_vec(),
            b"https://EXAMPLE.org/File?campaign=Normal".to_vec(),
            b"https://example.org/file?campaign=Normal".to_vec(),
            b"https://example.org/File?campaign=normal".to_vec(),
        ];
        assert_eq!(
            qr_urls_from_payloads(&payloads),
            [
                "https://example.org/File?campaign=Normal",
                "https://example.org/file?campaign=Normal",
                "https://example.org/File?campaign=normal",
            ]
        );
    }

    #[tokio::test]
    async fn timed_out_decode_keeps_the_single_cpu_slot_until_blocking_work_finishes() {
        let cpu_slot = Arc::new(tokio::sync::Semaphore::new(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let first_slot = Arc::clone(&cpu_slot);
        let first = tokio::spawn(async move {
            tokio::time::timeout(
                Duration::from_millis(20),
                run_bounded_qr_decode_with(first_slot, move || {
                    let _ = started_tx.send(());
                    release_rx.recv().expect("test releases blocking decoder");
                }),
            )
            .await
        });
        started_rx.await.expect("blocking decoder should start");
        assert!(first.await.expect("timeout wrapper should finish").is_err());

        let blocked = tokio::time::timeout(
            Duration::from_millis(20),
            run_bounded_qr_decode_with(Arc::clone(&cpu_slot), || ()),
        )
        .await;
        assert!(
            blocked.is_err(),
            "second decode must wait for the detached task"
        );

        release_tx.send(()).expect("decoder task is still waiting");
        let next = tokio::time::timeout(
            Duration::from_secs(1),
            run_bounded_qr_decode_with(cpu_slot, || 42),
        )
        .await
        .expect("CPU slot should be released when blocking work completes")
        .expect("second decoder task should join");
        assert_eq!(next, 42);
    }
}
