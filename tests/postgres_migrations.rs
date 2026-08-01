use std::{
    io::{ErrorKind, Read, Write},
    net::TcpListener,
    sync::{Arc, mpsc::sync_channel},
};

use chrono::{DateTime, Duration, TimeZone, Utc};
use sqlx::{PgPool, postgres::PgPoolOptions, query, query_as, query_scalar};
use teloxide::Bot;
use teloxide::utils::time::TimeContext;
use tg_ai_bot_teloxide::features::{
    ask::notes::add_user_note_from_search,
    ask::{
        repo::{CreateAskRunParams, RenderAudit, finish_delivery, finish_run},
        types::AskRunStatus,
    },
    chat_retrieval::{
        EmbeddingJob, claim_embedding_jobs, enqueue_message_embedding_if_enabled,
        mark_embedding_failed, mark_embedding_ready,
    },
    first_comment::repo::{
        CommentErrorKind, CreatePostCommentJobParams, FinalizePostCommentSent, LlmGenerationInsert,
        OperatorAuditParams, begin_post_comment_delivery,
        claim_delivery_unknown_post_comment_for_operator_retry, claim_next_post_comment_job,
        create_post_comment_job, finalize_post_comment_sent,
        mark_delivery_unknown_post_comment_delivered, mark_delivery_unknown_post_comment_failed,
        mark_operator_retry_post_comment_terminal_failed, mark_post_comment_delivery_unknown,
        mark_post_comment_pre_send_failed, mark_post_comment_send_rejected,
    },
    jobs::{claim::CasResult, observability::load_job_lifecycle_report},
    memory::service::{
        HistoryEntryCompletion, claim_next_history_entry, finalize_history_entry,
        finalize_history_failed, finalize_history_retry,
    },
    new_user_audit::{
        repo::{
            NewUserAuditJobParams, claim_next_new_user_audit_job, enqueue_new_user_audit_job,
            finalize_new_user_audit_job, mark_new_user_audit_failed,
            mark_new_user_audit_materialization_retry, mark_new_user_audit_materialization_stale,
            mark_new_user_audit_retry, materialize_new_user_audit_job,
        },
        scoring::ScoreComponents,
    },
    spam_review::{
        claim_next_review_delivery, create_review, mark_review_delivery_succeeded, send_review,
    },
    stats::{
        render_html, render_rich, repo as stats_repo,
        types::{AttractionMetrics, ChatStatsReportData, ReportWindow, StatsPeriod},
    },
};

#[tokio::test]
#[ignore = "run with ./scripts/test.sh against the local test database"]
async fn clean_test_database_applies_migrations_and_preserves_comment_job_lifecycle() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must be set by scripts/test.sh");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("local test database must be reachable");

    assert_clean_database_migrations(&pool).await;
    assert_ask_time_render_audit(&pool).await;
    assert_spam_review_safety_backfill_upgrade(&pool).await;
    assert_post_comment_delivery_lifecycle_upgrade(&pool).await;
    assert_sent_comment_requires_sent_at(&pool).await;
    assert_public_mcp_scope(&pool).await;
    assert_stats_renderers_share_period_data(&pool).await;
    assert_feature_gated_jobs(&pool).await;
    assert_agent_note_contract(&pool).await;
    assert_review_deduplication(&pool).await;
    assert_low_risk_review_delivery_is_blocked_by_database(&pool).await;
    assert_new_user_audit_job_lifecycle(&pool).await;
    assert_new_user_audit_generation_cas_requires_live_lease_and_current_version(&pool).await;
    assert_new_user_audit_enqueue_version_bump_reopens_completed_materialization(&pool).await;
    assert_new_user_audit_generation_finalizer_retries_real_transient_sqlstate(&database_url).await;
    assert_unified_enqueue_and_finalizer_share_lock_order(&pool).await;
    assert_successful_audit_replays_for_materialization(&pool).await;
    assert_audit_generation_is_durable_before_materialization(&pool).await;
    assert_new_user_audit_materialization_lifecycle(&pool).await;
    assert_new_user_audit_generation_materialization_upgrade(&pool).await;
    assert_review_delivery_finalization_requires_current_claim(&pool).await;
    assert_review_delivery_payload_cas_blocks_replaced_and_lowered_risk(&pool).await;
    assert_stale_review_delivery_failure_does_not_finalize_replaced_payload(&pool).await;
    assert_review_delivery_retry_uses_consecutive_failures(&pool).await;
    assert_terminal_review_delivery_stays_closed(&pool).await;
    assert_comment_job_lifecycle(&pool).await;
    assert_comment_reconciliation_requires_operator_claim(&pool).await;
    assert_embedding_job_finalization_requires_current_claim(&pool).await;
    assert_post_history_entry_lease_lifecycle(&pool).await;
    assert_job_lifecycle_observability(&pool).await;
}

async fn assert_ask_time_render_audit(pool: &PgPool) {
    #[derive(sqlx::FromRow)]
    struct AskTimeAuditRow {
        status: String,
        error_kind: Option<String>,
        render_captured_now: Option<DateTime<Utc>>,
        render_dialect: Option<String>,
        render_timezone: Option<String>,
        renderer_revision: Option<String>,
        rendered_markdown: Option<String>,
        render_version: Option<String>,
        delivery_certainty: Option<String>,
        delivery_outcome: Option<String>,
        answer_markdown: Option<String>,
    }

    let columns: Vec<String> = query_scalar(
        "select column_name from information_schema.columns where table_schema = 'public' and table_name = 'ask_runs' and column_name in ('render_captured_now', 'render_dialect', 'render_timezone', 'renderer_revision', 'rendered_markdown', 'render_version', 'delivery_certainty', 'delivery_outcome') order by column_name",
    )
    .fetch_all(pool)
    .await
    .expect("ask time render audit columns must be queryable");
    assert_eq!(
        columns,
        vec![
            "delivery_certainty".to_string(),
            "delivery_outcome".to_string(),
            "render_captured_now".to_string(),
            "render_dialect".to_string(),
            "render_timezone".to_string(),
            "render_version".to_string(),
            "rendered_markdown".to_string(),
            "renderer_revision".to_string(),
        ]
    );

    let suffix = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after unix epoch")
        .as_millis()
        % 1_000_000) as i32;
    let captured_now = Utc
        .with_ymd_and_hms(2026, 8, 2, 12, 34, 56)
        .single()
        .unwrap();
    let success_id = CreateAskRunParams {
        chat_id: -1001932061163,
        command_message_id: 9_800_000 + suffix,
        requester_user_id: 9_800_000 + i64::from(suffix),
        question: "time audit success",
        reply_to_message_id: None,
        provider: "test-provider",
        model: Some("test-model"),
    };
    let success_id = tg_ai_bot_teloxide::features::ask::repo::create_run(pool, success_id)
        .await
        .expect("success ask run must be created through the production repo");
    finish_run(
        pool,
        success_id,
        AskRunStatus::DeliveryPending,
        Some("source now/"),
        RenderAudit {
            captured_now: Some(captured_now),
            dialect: Some("llm-v1".to_owned()),
            timezone: Some("Europe/Moscow".to_owned()),
            renderer_revision: Some("time-rendering-v2".to_owned()),
            rendered_markdown: Some("compiled markdown".to_owned()),
            version: Some("test-app".to_owned()),
            delivery_certainty: None,
            delivery_outcome: Some("rich_delivered".to_owned()),
        },
        None,
    )
    .await
    .expect("success render audit must be written through finish_run");
    finish_delivery(
        pool,
        success_id,
        AskRunStatus::Completed,
        "rich_delivered",
        None,
    )
    .await
    .expect("success delivery outcome must be finalized through the repo");
    let success: AskTimeAuditRow = query_as(
        "select status, error_kind, render_captured_now, render_dialect, render_timezone, renderer_revision, rendered_markdown, render_version, delivery_certainty, delivery_outcome, answer_markdown from ask_runs where id = $1",
    )
    .bind(success_id)
    .fetch_one(pool)
    .await
    .expect("success render audit must round-trip");
    assert_eq!(success.status, "completed");
    assert_eq!(success.error_kind, None);
    assert_eq!(success.render_captured_now, Some(captured_now));
    assert_eq!(success.render_dialect.as_deref(), Some("llm-v1"));
    assert_eq!(success.render_timezone.as_deref(), Some("Europe/Moscow"));
    assert_eq!(
        success.renderer_revision.as_deref(),
        Some("time-rendering-v2")
    );
    assert_eq!(
        success.rendered_markdown.as_deref(),
        Some("compiled markdown")
    );
    assert_eq!(success.render_version.as_deref(), Some("test-app"));
    assert_eq!(success.delivery_certainty, None);
    assert_eq!(success.delivery_outcome.as_deref(), Some("rich_delivered"));
    assert_eq!(success.answer_markdown.as_deref(), Some("source now/"));

    let error_id = tg_ai_bot_teloxide::features::ask::repo::create_run(
        pool,
        CreateAskRunParams {
            chat_id: -1001932061163,
            command_message_id: 9_900_000 + suffix,
            requester_user_id: 9_900_000 + i64::from(suffix),
            question: "time audit error",
            reply_to_message_id: None,
            provider: "test-provider",
            model: None,
        },
    )
    .await
    .expect("error ask run must be created through the production repo");
    finish_run(
        pool,
        error_id,
        AskRunStatus::Failed,
        Some("source now+3hours/"),
        RenderAudit {
            captured_now: Some(captured_now),
            dialect: Some("llm-v1".to_owned()),
            timezone: Some("Europe/Moscow".to_owned()),
            renderer_revision: Some("time-rendering-v2".to_owned()),
            rendered_markdown: None,
            version: Some("test-app".to_owned()),
            delivery_certainty: Some(teloxide::drafter::DeliveryCertainty::NotAttempted),
            delivery_outcome: Some("render_failed".to_owned()),
        },
        Some("render_validation"),
    )
    .await
    .expect("failed render audit must be written through finish_run");
    let error: AskTimeAuditRow = query_as(
        "select status, error_kind, render_captured_now, render_dialect, render_timezone, renderer_revision, rendered_markdown, render_version, delivery_certainty, delivery_outcome, answer_markdown from ask_runs where id = $1",
    )
    .bind(error_id)
    .fetch_one(pool)
    .await
    .expect("failed render audit must round-trip");
    assert_eq!(error.status, "failed");
    assert_eq!(error.error_kind.as_deref(), Some("render_validation"));
    assert_eq!(error.render_captured_now, Some(captured_now));
    assert_eq!(error.render_dialect.as_deref(), Some("llm-v1"));
    assert_eq!(error.render_timezone.as_deref(), Some("Europe/Moscow"));
    assert_eq!(
        error.renderer_revision.as_deref(),
        Some("time-rendering-v2")
    );
    assert_eq!(error.rendered_markdown, None);
    assert_eq!(error.render_version.as_deref(), Some("test-app"));
    assert_eq!(error.delivery_certainty.as_deref(), Some("not_attempted"));
    assert_eq!(error.delivery_outcome.as_deref(), Some("render_failed"));
    assert_eq!(error.answer_markdown.as_deref(), Some("source now+3hours/"));
}

async fn assert_job_lifecycle_observability(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const MESSAGE_ID: i32 = 9_300_003;
    const HIGH_RISK_USER_ID: i64 = 9_000_101;
    const LOW_RISK_USER_ID: i64 = 9_000_102;
    query(
        "update telegram_message_embeddings set error_kind = 'embedding_batch_cardinality' where chat_id = $1 and message_id = $2",
    )
    .bind(CHAT_ID)
    .bind(MESSAGE_ID)
    .execute(pool)
    .await
    .expect("embedding cardinality fixture must be recorded");
    query(
        r#"
        insert into spam_review_requests
            (chat_id, telegram_user_id, risk_score, notification_status, notification_next_attempt_at)
        values
            ($1, $2, 80, 'pending', now() - interval '1 minute'),
            ($1, $3, 65, 'pending', now() - interval '1 day')
        "#,
    )
    .bind(CHAT_ID)
    .bind(HIGH_RISK_USER_ID)
    .bind(LOW_RISK_USER_ID)
    .execute(pool)
    .await
    .expect("high- and low-risk ready review fixtures must be inserted");

    let report = load_job_lifecycle_report(pool)
        .await
        .expect("read-only lifecycle report query must succeed");
    assert_eq!(
        report.iter().map(|queue| queue.queue).collect::<Vec<_>>(),
        vec!["first-comments", "embeddings", "post-history", "reviews"]
    );
    let embeddings = report
        .iter()
        .find(|queue| queue.queue == "embeddings")
        .expect("embeddings queue must be reported");
    assert_eq!(embeddings.embedding_batch_cardinality_failures, Some(1));
    assert!(embeddings.lease_reclaim_count >= 1);
    assert!(embeddings.errors.iter().any(|metric| {
        metric.error_kind == "embedding_batch_cardinality"
            && metric.jobs == 1
            && metric.terminal_failures == 1
    }));
    let first_comments = report
        .iter()
        .find(|queue| queue.queue == "first-comments")
        .expect("first-comments queue must be reported");
    assert!(first_comments.lease_reclaim_count >= 1);
    assert!(first_comments.errors.iter().any(|metric| {
        metric.error_kind == "operator_marked_failed"
            && metric.jobs == 1
            && metric.terminal_failures == 1
    }));
    let history = report
        .iter()
        .find(|queue| queue.queue == "post-history")
        .expect("post-history queue must be reported");
    assert!(history.lease_reclaim_count >= 1);

    let reviews = report
        .iter()
        .find(|queue| queue.queue == "reviews")
        .expect("reviews queue must be reported");
    assert!(reviews.lease_reclaim_count >= 1);
    let oldest_ready_age = reviews
        .oldest_ready_age_seconds
        .expect("due high-risk review must contribute to oldest_ready_age");
    assert!(
        oldest_ready_age < 3_600.0,
        "an old low-risk review must be excluded from oldest_ready_age: {oldest_ready_age}"
    );
}

async fn assert_post_history_entry_lease_lifecycle(pool: &PgPool) {
    const STAGED_JOB: i32 = 9_500_001;
    const SUCCESS_JOB: i32 = 9_500_002;
    const RETRY_JOB: i32 = 9_500_003;
    const FAILED_JOB: i32 = 9_500_004;
    const RECLAIM_JOB: i32 = 9_500_005;

    let staged_id = create_job(pool, STAGED_JOB).await;
    query(
        "insert into post_history_entries (post_comment_job_id, source_channel_id, source_message_id, post_text, bot_comment, status, attempts, processing_started_at, lease_expires_at) values ($1, -1001575496091, $2, 'lease lifecycle post', 'lease lifecycle comment', 'processing', 1, null, null)",
    )
    .bind(staged_id)
    .bind(STAGED_JOB)
    .execute(pool)
    .await
    .expect("staged processing history row must be inserted");
    query("drop index if exists public.post_history_entries_processing_lease_idx")
        .execute(pool)
        .await
        .expect("processing lease index must be removable for staged upgrade");
    sqlx::raw_sql(include_str!(
        "../migrations/20260729111000_post_history_entry_leases.sql"
    ))
    .execute(pool)
    .await
    .expect("post history lease migration must upgrade staged processing rows");
    let staged_lease_expired: bool = query_scalar(
        "select lease_expires_at < now() from post_history_entries where post_comment_job_id = $1 and processing_started_at is null",
    )
    .bind(staged_id)
    .fetch_one(pool)
    .await
    .expect("legacy row without processing start must receive an expired lease");
    assert!(staged_lease_expired);
    let lease_index: Option<String> = query_scalar(
        "select indexname from pg_indexes where schemaname = 'public' and tablename = 'post_history_entries' and indexname = 'post_history_entries_processing_lease_idx'",
    )
    .fetch_one(pool)
    .await
    .expect("processing lease index lookup must succeed");
    assert_eq!(
        lease_index.as_deref(),
        Some("post_history_entries_processing_lease_idx")
    );
    query("update post_history_entries set status = 'failed' where id = $1")
        .bind(staged_id)
        .execute(pool)
        .await
        .expect("staged migration fixture must be closed");

    let success_id = create_history_fixture(pool, SUCCESS_JOB).await;
    let success_claim = claim_history_entry(pool, success_id, 1).await;
    expire_history_lease(pool, success_id).await;
    assert_eq!(
        finalize_history_entry(pool, &success_claim, ready_history_completion())
            .await
            .expect("expired, unreclaimed success finalizer must execute"),
        CasResult::Applied
    );
    assert_history_state(pool, success_id, "ready", false).await;

    let retry_id = create_history_fixture(pool, RETRY_JOB).await;
    let retry_claim = claim_history_entry(pool, retry_id, 1).await;
    expire_history_lease(pool, retry_id).await;
    assert_eq!(
        finalize_history_retry(pool, &retry_claim, "test_retry")
            .await
            .expect("expired, unreclaimed retry finalizer must execute"),
        CasResult::Applied
    );
    assert_history_state(pool, retry_id, "retry", true).await;

    let failed_id = create_history_fixture(pool, FAILED_JOB).await;
    let failed_claim = claim_history_entry(pool, failed_id, 1).await;
    expire_history_lease(pool, failed_id).await;
    assert_eq!(
        finalize_history_failed(pool, &failed_claim, "test_failure")
            .await
            .expect("expired, unreclaimed failed finalizer must execute"),
        CasResult::Applied
    );
    assert_history_state(pool, failed_id, "failed", false).await;

    let reclaim_id = create_history_fixture(pool, RECLAIM_JOB).await;
    let first_claim = claim_history_entry(pool, reclaim_id, 1).await;
    expire_history_lease(pool, reclaim_id).await;
    let second_claim = claim_history_entry(pool, reclaim_id, 2).await;
    assert_eq!(second_claim.id, first_claim.id);
    assert_eq!(
        finalize_history_entry(pool, &first_claim, ready_history_completion())
            .await
            .expect("stale history finalizer must execute"),
        CasResult::LeaseLost
    );
    assert_eq!(
        finalize_history_entry(pool, &second_claim, ready_history_completion())
            .await
            .expect("current reclaimed history finalizer must execute"),
        CasResult::Applied
    );
    assert_history_state(pool, reclaim_id, "ready", false).await;
    let reclaim_count: i32 =
        query_scalar("select lease_reclaim_count from post_history_entries where id = $1")
            .bind(reclaim_id)
            .fetch_one(pool)
            .await
            .expect("reclaimed history row must expose its reclaim count");
    assert_eq!(reclaim_count, 1);
}

async fn create_history_fixture(pool: &PgPool, sequence: i32) -> i64 {
    let post_comment_job_id = create_job(pool, sequence).await;
    let id: i64 = query_scalar(
        "insert into post_history_entries (post_comment_job_id, source_channel_id, source_message_id, post_text, bot_comment, next_attempt_at) values ($1, -1001575496091, $2, 'lease lifecycle post', 'lease lifecycle comment', now() - interval '1 day') returning id",
    )
    .bind(post_comment_job_id)
    .bind(sequence)
    .fetch_one(pool)
    .await
    .expect("post history lifecycle fixture must be inserted");
    id
}

async fn claim_history_entry(
    pool: &PgPool,
    expected_id: i64,
    expected_attempts: i32,
) -> tg_ai_bot_teloxide::features::memory::service::HistoryEntryClaim {
    let claim = claim_next_history_entry(pool)
        .await
        .expect("history claim must execute")
        .expect("history fixture must be claimable");
    assert_eq!(claim.id, expected_id);
    assert_eq!(claim.attempts, expected_attempts);
    claim
}

async fn expire_history_lease(pool: &PgPool, id: i64) {
    query("update post_history_entries set lease_expires_at = now() - interval '1 second' where id = $1")
        .bind(id)
        .execute(pool)
        .await
        .expect("history lease must expire for lifecycle test");
}

fn ready_history_completion() -> HistoryEntryCompletion {
    HistoryEntryCompletion {
        summary: Some("A reusable post history summary for lifecycle tests.".to_string()),
        entities: vec!["fixture".to_string()],
        used_angle: Some("lifecycle".to_string()),
        external_fact: None,
        skip_reason: None,
        provider: "test".to_string(),
        model: "test".to_string(),
        embedding: Some(vec![0.0; 312]),
        embedding_model: "test".to_string(),
    }
}

async fn assert_history_state(pool: &PgPool, id: i64, expected_status: &str, retry: bool) {
    let state: (String, bool, bool, bool) = query_as(
        "select status, processing_started_at is not null, lease_expires_at is not null, next_attempt_at > now() from post_history_entries where id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("finalized history row must remain queryable");
    assert_eq!(state, (expected_status.to_string(), false, false, retry));
}

async fn assert_embedding_job_finalization_requires_current_claim(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const STALE_MESSAGE_ID: i32 = 9_300_001;
    const RETRY_MESSAGE_ID: i32 = 9_300_002;
    const FAILED_MESSAGE_ID: i32 = 9_300_003;
    const TEXT: &str = "embedding finalization CAS";
    const USER_ID: i64 = 9_300_000;

    query("insert into telegram_user_profiles (telegram_user_id, first_name) values ($1, 'Embedding CAS')")
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("embedding source profile must be inserted");
    for message_id in [STALE_MESSAGE_ID, RETRY_MESSAGE_ID, FAILED_MESSAGE_ID] {
        query("insert into telegram_messages (chat_id, message_id, user_id, text) values ($1, $2, $3, $4)")
            .bind(CHAT_ID)
            .bind(message_id)
            .bind(USER_ID)
            .bind(TEXT)
            .execute(pool)
            .await
            .expect("embedding source message must be inserted");
    }
    query(
        "insert into telegram_message_embeddings (chat_id, message_id, status, attempts, processing_started_at, lease_expires_at) values ($1, $2, 'processing', 1, now(), now() - interval '1 second')",
    )
    .bind(CHAT_ID)
    .bind(STALE_MESSAGE_ID)
    .execute(pool)
    .await
    .expect("reclaimed embedding job must be inserted");

    let stale_claim = EmbeddingJob {
        chat_id: CHAT_ID,
        message_id: STALE_MESSAGE_ID,
        text: TEXT.to_string(),
        attempts: 1,
    };
    query(
        "update telegram_message_embeddings set status = 'ignored' where status in ('pending', 'retry_wait') and (chat_id, message_id) <> ($1, $2)",
    )
    .bind(CHAT_ID)
    .bind(STALE_MESSAGE_ID)
    .execute(pool)
    .await
    .expect("unrelated pending embedding fixtures must not affect the reclaim regression");
    let reclaimed_jobs = claim_embedding_jobs(pool, 1)
        .await
        .expect("expired embedding claim must execute");
    assert_eq!(reclaimed_jobs.len(), 1);
    let current_claim = &reclaimed_jobs[0];
    assert_eq!(current_claim.chat_id, CHAT_ID);
    assert_eq!(current_claim.message_id, STALE_MESSAGE_ID);
    assert_eq!(current_claim.attempts, 2);
    assert_eq!(
        mark_embedding_ready(pool, &stale_claim, &vec![0.0; 312], "test-model")
            .await
            .expect("stale ready finalization must execute"),
        tg_ai_bot_teloxide::features::jobs::claim::CasResult::LeaseLost
    );
    assert_eq!(
        mark_embedding_failed(pool, &stale_claim, "test_failure")
            .await
            .expect("stale failure finalization must execute"),
        tg_ai_bot_teloxide::features::jobs::claim::CasResult::LeaseLost
    );
    let reclaimed_state: (String, i32, bool, bool) = query_as(
        "select status, attempts, processing_started_at is not null, lease_expires_at is not null from telegram_message_embeddings where chat_id = $1 and message_id = $2",
    )
    .bind(CHAT_ID)
    .bind(STALE_MESSAGE_ID)
    .fetch_one(pool)
    .await
    .expect("reclaimed embedding job must remain stored");
    assert_eq!(reclaimed_state, ("processing".to_string(), 2, true, true));

    assert_eq!(
        mark_embedding_ready(pool, current_claim, &vec![0.0; 312], "test-model")
            .await
            .expect("current ready finalization must execute"),
        tg_ai_bot_teloxide::features::jobs::claim::CasResult::Applied
    );
    let ready_state: (String, bool, bool) = query_as(
        "select status, processing_started_at is not null, lease_expires_at is not null from telegram_message_embeddings where chat_id = $1 and message_id = $2",
    )
    .bind(CHAT_ID)
    .bind(STALE_MESSAGE_ID)
    .fetch_one(pool)
    .await
    .expect("ready embedding job must remain stored");
    assert_eq!(ready_state, ("ready".to_string(), false, false));

    assert_embedding_failure_clears_claim(pool, CHAT_ID, RETRY_MESSAGE_ID, 1, "retry_wait").await;
    assert_embedding_failure_clears_claim(pool, CHAT_ID, FAILED_MESSAGE_ID, 5, "failed").await;
}

async fn assert_embedding_failure_clears_claim(
    pool: &PgPool,
    chat_id: i64,
    message_id: i32,
    attempts: i32,
    expected_status: &str,
) {
    query(
        "insert into telegram_message_embeddings (chat_id, message_id, status, attempts, processing_started_at, lease_expires_at) values ($1, $2, 'processing', $3, now(), now() + interval '10 minutes')",
    )
    .bind(chat_id)
    .bind(message_id)
    .bind(attempts)
    .execute(pool)
    .await
    .expect("processing embedding job must be inserted");
    let job = EmbeddingJob {
        chat_id,
        message_id,
        text: "embedding finalization CAS".to_string(),
        attempts,
    };
    assert_eq!(
        mark_embedding_failed(pool, &job, "test_failure")
            .await
            .expect("current failure finalization must execute"),
        tg_ai_bot_teloxide::features::jobs::claim::CasResult::Applied
    );
    let state: (String, Option<String>, bool, bool) = query_as(
        "select status, error_kind, processing_started_at is not null, lease_expires_at is not null from telegram_message_embeddings where chat_id = $1 and message_id = $2",
    )
    .bind(chat_id)
    .bind(message_id)
    .fetch_one(pool)
    .await
    .expect("failed embedding job must remain stored");
    assert_eq!(
        state,
        (
            expected_status.to_string(),
            Some("test_failure".to_string()),
            false,
            false
        )
    );
}

async fn assert_spam_review_safety_backfill_upgrade(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const HISTORICAL_USER_ID: i64 = 9_000_010;
    const CURRENT_USER_ID: i64 = 9_000_011;
    let lifecycle_installed_on: chrono::DateTime<Utc> =
        query_scalar("select installed_on from _sqlx_migrations where version = 20260729100000")
            .fetch_one(pool)
            .await
            .expect("lifecycle migration timestamp must exist");
    for (user_id, notified_at) in [
        (
            HISTORICAL_USER_ID,
            lifecycle_installed_on - Duration::seconds(1),
        ),
        (
            CURRENT_USER_ID,
            lifecycle_installed_on + Duration::seconds(1),
        ),
    ] {
        query("insert into spam_review_requests (chat_id, telegram_user_id, risk_score, risk_signals, notified_at, notification_status, notification_attempts, notified_risk_score, notified_risk_signals) values ($1, $2, 80, '[{\"label\": \"fixture\"}]'::jsonb, $3, 'pending', 0, null, null)")
            .bind(CHAT_ID)
            .bind(user_id)
            .bind(notified_at)
            .execute(pool)
            .await
            .expect("review upgrade fixture must be inserted");
    }
    query("update spam_review_requests set risk_score = 65 where chat_id = $1 and telegram_user_id = $2")
        .bind(CHAT_ID)
        .bind(CURRENT_USER_ID)
        .execute(pool)
        .await
        .expect("post-lifecycle fixture must not be claimable by high-risk delivery worker");
    sqlx::raw_sql(include_str!(
        "../migrations/20260729102000_spam_review_delivery_safety.sql"
    ))
    .execute(pool)
    .await
    .expect("safety migration must backfill the staged upgrade fixture");
    let historical: (String, Option<i32>, Option<serde_json::Value>) = query_as(
        "select notification_status, notified_risk_score, notified_risk_signals from spam_review_requests where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(HISTORICAL_USER_ID)
    .fetch_one(pool)
    .await
    .expect("historical review fixture must remain queryable");
    assert_eq!(historical.0, "sent");
    assert_eq!(historical.1, Some(80));
    assert_eq!(
        historical.2,
        Some(serde_json::json!([{"label": "fixture"}]))
    );
    let current: (String, Option<i32>) = query_as(
        "select notification_status, notified_risk_score from spam_review_requests where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(CURRENT_USER_ID)
    .fetch_one(pool)
    .await
    .expect("post-lifecycle review fixture must remain queryable");
    assert_eq!(current, ("pending".into(), None));
}

async fn assert_post_comment_delivery_lifecycle_upgrade(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const SOURCE_CHANNEL_ID: i64 = -1001575496091;
    const PROCESSING_MESSAGE_ID: i32 = 9_400_001;
    const PENDING_MESSAGE_ID: i32 = 9_400_002;
    const RETRY_WAIT_MESSAGE_ID: i32 = 9_400_003;
    const SENT_MESSAGE_ID: i32 = 9_400_004;
    const FAILED_MESSAGE_ID: i32 = 9_400_005;

    for (message_id, status) in [
        (PROCESSING_MESSAGE_ID, "processing"),
        (PENDING_MESSAGE_ID, "pending"),
        (RETRY_WAIT_MESSAGE_ID, "retry_wait"),
        (SENT_MESSAGE_ID, "sent"),
        (FAILED_MESSAGE_ID, "failed"),
    ] {
        query(
            r#"
            insert into post_comment_jobs
                (discussion_chat_id, discussion_message_id, source_channel_id, source_message_id,
                 cleaned_post_text, status, sent_at)
            values ($1, $2, $3, $2, 'delivery lifecycle upgrade fixture', $4,
                    case when $4 = 'sent' then now() else null end)
            "#,
        )
        .bind(CHAT_ID)
        .bind(message_id)
        .bind(SOURCE_CHANNEL_ID)
        .bind(status)
        .execute(pool)
        .await
        .expect("staged delivery lifecycle fixture must be inserted");
    }

    query("drop index if exists public.post_comment_jobs_sending_lease_idx")
        .execute(pool)
        .await
        .expect("sending lease index must be removable for staged upgrade");
    query("drop index if exists public.post_comment_jobs_delivery_unknown_idx")
        .execute(pool)
        .await
        .expect("delivery unknown index must be removable for staged upgrade");
    query("drop index if exists public.llm_generations_post_comment_job_id_unique")
        .execute(pool)
        .await
        .expect("generation idempotency index must be removable for staged upgrade");
    sqlx::raw_sql(include_str!(
        "../migrations/20260729110000_post_comment_delivery_lifecycle.sql"
    ))
    .execute(pool)
    .await
    .expect("delivery lifecycle migration must upgrade staged legacy rows");

    let statuses: Vec<(i32, String)> = query_as(
        r#"
        select discussion_message_id, status
        from post_comment_jobs
        where discussion_chat_id = $1
          and discussion_message_id = any($2)
        order by discussion_message_id
        "#,
    )
    .bind(CHAT_ID)
    .bind([
        PROCESSING_MESSAGE_ID,
        PENDING_MESSAGE_ID,
        RETRY_WAIT_MESSAGE_ID,
        SENT_MESSAGE_ID,
        FAILED_MESSAGE_ID,
    ])
    .fetch_all(pool)
    .await
    .expect("upgraded delivery lifecycle fixtures must remain queryable");
    assert_eq!(
        statuses,
        vec![
            (PROCESSING_MESSAGE_ID, "delivery_unknown".to_string()),
            (PENDING_MESSAGE_ID, "pending".to_string()),
            (RETRY_WAIT_MESSAGE_ID, "retry_wait".to_string()),
            (SENT_MESSAGE_ID, "sent".to_string()),
            (FAILED_MESSAGE_ID, "failed".to_string()),
        ]
    );
    query(
        "delete from post_comment_jobs where discussion_chat_id = $1 and discussion_message_id = any($2)",
    )
    .bind(CHAT_ID)
    .bind([
        PROCESSING_MESSAGE_ID,
        PENDING_MESSAGE_ID,
        RETRY_WAIT_MESSAGE_ID,
        SENT_MESSAGE_ID,
        FAILED_MESSAGE_ID,
    ])
    .execute(pool)
    .await
    .expect("staged delivery lifecycle fixtures must be cleaned up");
}

async fn assert_low_risk_review_delivery_is_blocked_by_database(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_099;
    query(
        "insert into spam_review_requests (chat_id, telegram_user_id, risk_score, risk_signals) values ($1, $2, 69, '[]'::jsonb)",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("low-risk audit snapshot must be stored");

    let error = query(
        "update spam_review_requests set notification_attempts = 1, notification_status = 'processing' where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect_err("database must reject a low-risk delivery claim");
    assert!(
        error.to_string().contains(
            "cannot transition spam review request into processing with risk_score below 70"
        ),
        "unexpected database error: {error}"
    );
}

async fn assert_new_user_audit_job_lifecycle(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_098;
    let input = serde_json::json!({"schema_version": "fixture-v1"});

    let params = NewUserAuditJobParams {
        chat_id: CHAT_ID,
        telegram_user_id: USER_ID,
        snapshot_hash: "snapshot-a",
        prompt_version: "prompt-v1",
        input_json: &input,
        avatar_file_id: None,
        avatar_file_unique_id: None,
    };
    enqueue_new_user_audit_job(pool, params)
        .await
        .expect("new audit job enqueue must succeed");
    enqueue_new_user_audit_job(
        pool,
        NewUserAuditJobParams {
            chat_id: CHAT_ID,
            telegram_user_id: USER_ID,
            snapshot_hash: "snapshot-a",
            prompt_version: "prompt-v1",
            input_json: &input,
            avatar_file_id: None,
            avatar_file_unique_id: None,
        },
    )
    .await
    .expect("identical audit job enqueue must be idempotent");

    let count: i64 = query_scalar(
        "select count(*) from new_user_audit_jobs where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("audit job dedup count must be queryable");
    assert_eq!(count, 1);

    let first_claim = claim_next_new_user_audit_job(pool)
        .await
        .expect("first audit claim must execute")
        .expect("audit job must be claimed");
    query("update new_user_audit_jobs set lease_expires_at = now() - interval '1 second' where id = $1")
        .bind(first_claim.id)
        .execute(pool)
        .await
        .expect("audit lease must expire");

    let second_claim = claim_next_new_user_audit_job(pool)
        .await
        .expect("reclaimed audit claim must execute")
        .expect("expired audit job must be reclaimed");
    assert!(second_claim.attempts > first_claim.attempts);
    assert_eq!(
        mark_new_user_audit_failed(pool, &first_claim, "fixture_stale")
            .await
            .expect("stale audit finalizer must execute"),
        CasResult::LeaseLost
    );
    assert_eq!(
        mark_new_user_audit_retry(pool, &second_claim, "fixture_retry", None)
            .await
            .expect("current audit retry finalizer must execute"),
        CasResult::Applied
    );

    let retry_state: (String, i32) =
        query_as("select status, attempts from new_user_audit_jobs where id = $1")
            .bind(second_claim.id)
            .fetch_one(pool)
            .await
            .expect("audit retry state must be stored");
    assert_eq!(
        retry_state,
        ("retry_wait".to_string(), second_claim.attempts)
    );

    query("update new_user_audit_jobs set next_attempt_at = now() where id = $1")
        .bind(second_claim.id)
        .execute(pool)
        .await
        .expect("audit retry must be made due");
    let final_claim = claim_next_new_user_audit_job(pool)
        .await
        .expect("final audit claim must execute")
        .expect("audit retry must be claimed");
    assert_eq!(
        mark_new_user_audit_failed(pool, &final_claim, "fixture_terminal")
            .await
            .expect("terminal audit finalizer must execute"),
        CasResult::Applied
    );
    let terminal_status: String =
        query_scalar("select status from new_user_audit_jobs where id = $1")
            .bind(final_claim.id)
            .fetch_one(pool)
            .await
            .expect("terminal audit status must be stored");
    assert_eq!(terminal_status, "failed");
}

/// Regression coverage for the authoritative `job → audit → review` lock order.
/// The first transaction deliberately holds the job lock while the second begins;
/// a reversed audit-first enqueue would form a deadlock cycle with a finalizer.
async fn assert_new_user_audit_generation_cas_requires_live_lease_and_current_version(
    pool: &PgPool,
) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_091;
    let input = serde_json::json!({"schema_version": "generation-cas-fixture"});
    let params = NewUserAuditJobParams {
        chat_id: CHAT_ID,
        telegram_user_id: USER_ID,
        snapshot_hash: "generation-cas-snapshot",
        prompt_version: "prompt-v1",
        input_json: &input,
        avatar_file_id: None,
        avatar_file_unique_id: None,
    };
    let assessment = serde_json::json!({"fixture": "assessment"});

    enqueue_new_user_audit_job(pool, params)
        .await
        .expect("generation CAS fixture must be enqueued");
    let job_id: i64 = query_scalar(
        "select id from new_user_audit_jobs where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("generation CAS fixture id must be queryable");
    query(
        "update new_user_audit_jobs set materialization_version = 'obsolete-version' where id = $1",
    )
    .bind(job_id)
    .execute(pool)
    .await
    .expect("obsolete materialization version must be set");
    enqueue_new_user_audit_job(pool, params)
        .await
        .expect("conflicting enqueue must restore the current materialization version");
    let enqueued_version: String =
        query_scalar("select materialization_version from new_user_audit_jobs where id = $1")
            .bind(job_id)
            .fetch_one(pool)
            .await
            .expect("enqueued materialization version must be queryable");
    assert_eq!(
        enqueued_version,
        tg_ai_bot_teloxide::features::new_user_audit::repo::CURRENT_MATERIALIZATION_VERSION
    );

    let expired_claim = claim_next_new_user_audit_job(pool)
        .await
        .expect("generation CAS expired claim must execute")
        .expect("generation CAS fixture must be claimable");
    query("update new_user_audit_jobs set lease_expires_at = now() - interval '1 second' where id = $1")
        .bind(expired_claim.id)
        .execute(pool)
        .await
        .expect("generation CAS lease must expire");
    assert_eq!(
        finalize_new_user_audit_job(
            pool,
            &expired_claim,
            tg_ai_bot_teloxide::features::new_user_audit::repo::NewUserAuditOutcome {
                assessment_json: &assessment,
                provider: "fixture",
                model: "fixture",
            },
        )
        .await
        .expect("expired generation finalizer must execute"),
        CasResult::LeaseLost
    );

    let current_claim = claim_next_new_user_audit_job(pool)
        .await
        .expect("reclaimed generation CAS claim must execute")
        .expect("expired generation CAS fixture must be reclaimable");
    query(
        "update new_user_audit_jobs set materialization_version = 'obsolete-version' where id = $1",
    )
    .bind(current_claim.id)
    .execute(pool)
    .await
    .expect("obsolete finalizer materialization version must be set");
    assert_eq!(
        finalize_new_user_audit_job(
            pool,
            &current_claim,
            tg_ai_bot_teloxide::features::new_user_audit::repo::NewUserAuditOutcome {
                assessment_json: &assessment,
                provider: "fixture",
                model: "fixture",
            },
        )
        .await
        .expect("current generation finalizer must execute"),
        CasResult::Applied
    );
    let finalized_version: String =
        query_scalar("select materialization_version from new_user_audit_jobs where id = $1")
            .bind(current_claim.id)
            .fetch_one(pool)
            .await
            .expect("finalized materialization version must be queryable");
    assert_eq!(
        finalized_version,
        tg_ai_bot_teloxide::features::new_user_audit::repo::CURRENT_MATERIALIZATION_VERSION
    );
    query(
        "update new_user_audit_jobs set materialization_status = 'succeeded', materialized_at = now(), materialization_next_attempt_at = now() + interval '1 day' where id = $1",
    )
    .bind(current_claim.id)
    .execute(pool)
    .await
    .expect("generation CAS replay fixture must be closed");
}

async fn assert_new_user_audit_enqueue_version_bump_reopens_completed_materialization(
    pool: &PgPool,
) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_110;
    let input = serde_json::json!({"schema_version": "version-bump-fixture"});
    let params = NewUserAuditJobParams {
        chat_id: CHAT_ID,
        telegram_user_id: USER_ID,
        snapshot_hash: "version-bump-snapshot",
        prompt_version: "prompt-v1",
        input_json: &input,
        avatar_file_id: None,
        avatar_file_unique_id: None,
    };

    enqueue_new_user_audit_job(pool, params)
        .await
        .expect("version bump fixture must be enqueued");
    query(
        "update new_user_audit_jobs set status = 'succeeded', assessment_json = '{\"fixture\": \"assessment\"}'::jsonb, materialization_version = 'obsolete-version', materialization_status = 'succeeded', materialization_attempts = 5, materialization_next_attempt_at = now() + interval '1 day', materialization_processing_started_at = now(), materialization_lease_expires_at = now() + interval '1 hour', materialization_error_kind = 'obsolete_failure', materialized_at = now() where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("completed obsolete materialization fixture must be stored");

    enqueue_new_user_audit_job(pool, params)
        .await
        .expect("conflicting enqueue must reopen obsolete materialization");
    let state: (String, i32, bool, bool, bool, Option<String>, bool) = query_as(
        "select materialization_status, materialization_attempts, materialization_next_attempt_at <= now(), materialization_processing_started_at is null, materialization_lease_expires_at is null, materialization_error_kind, materialized_at is null from new_user_audit_jobs where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("reopened materialization state must be queryable");
    assert_eq!(
        state,
        ("pending".into(), 0, true, true, true, None, true),
        "a materialization version bump must reset the completed replay lifecycle"
    );
    let job_id: i64 = query_scalar(
        "select id from new_user_audit_jobs where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("version bump fixture id must be queryable");
    query(
        "update new_user_audit_jobs set materialization_status = 'succeeded', materialized_at = now(), materialization_next_attempt_at = now() + interval '1 day' where id = $1",
    )
    .bind(job_id)
    .execute(pool)
    .await
    .expect("version bump replay fixture must be closed");
}

async fn assert_new_user_audit_generation_finalizer_retries_real_transient_sqlstate(
    database_url: &str,
) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_111;

    let retry_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await
        .expect("retry test database pool must connect");
    let input = serde_json::json!({"schema_version": "generation-retry-fixture"});
    let assessment = serde_json::json!({"fixture": "retry-assessment"});

    enqueue_new_user_audit_job(
        &retry_pool,
        NewUserAuditJobParams {
            chat_id: CHAT_ID,
            telegram_user_id: USER_ID,
            snapshot_hash: "generation-retry-snapshot",
            prompt_version: "prompt-v1",
            input_json: &input,
            avatar_file_id: None,
            avatar_file_unique_id: None,
        },
    )
    .await
    .expect("generation retry fixture must be enqueued");
    let claim = claim_next_new_user_audit_job(&retry_pool)
        .await
        .expect("generation retry claim must execute")
        .expect("generation retry job must be claimable");

    query("drop trigger if exists new_user_audit_generation_retry_timeout on new_user_audit_jobs")
        .execute(&retry_pool)
        .await
        .expect("previous retry trigger must be removable");
    query("drop function if exists new_user_audit_generation_retry_timeout()")
        .execute(&retry_pool)
        .await
        .expect("previous retry trigger function must be removable");
    query("drop sequence if exists new_user_audit_generation_retry_calls")
        .execute(&retry_pool)
        .await
        .expect("previous retry sequence must be removable");
    query("create sequence new_user_audit_generation_retry_calls")
        .execute(&retry_pool)
        .await
        .expect("retry sequence must be created");
    query(
        "create function new_user_audit_generation_retry_timeout() returns trigger language plpgsql as $$ begin perform nextval('new_user_audit_generation_retry_calls'); perform pg_sleep(0.2); return new; end; $$",
    )
    .execute(&retry_pool)
    .await
    .expect("retry trigger function must be created");
    query(
        "create trigger new_user_audit_generation_retry_timeout before update on new_user_audit_jobs for each row execute function new_user_audit_generation_retry_timeout()",
    )
    .execute(&retry_pool)
    .await
    .expect("retry trigger must be created");

    let mut connection = retry_pool
        .acquire()
        .await
        .expect("retry pool connection must be acquired");
    query("set statement_timeout = '50ms'")
        .execute(&mut *connection)
        .await
        .expect("retry connection timeout must be configured");
    drop(connection);

    let error = finalize_new_user_audit_job(
        &retry_pool,
        &claim,
        tg_ai_bot_teloxide::features::new_user_audit::repo::NewUserAuditOutcome {
            assessment_json: &assessment,
            provider: "fixture",
            model: "fixture",
        },
    )
    .await
    .expect_err("cancelled finalization must exhaust its local transient retries");
    assert!(
        error
            .downcast_ref::<sqlx::Error>()
            .and_then(|error| error.as_database_error())
            .and_then(|error| error.code())
            .is_some_and(|code| code.as_ref() == "57014"),
        "expected PostgreSQL query-cancel SQLSTATE after transient retries: {error:#}"
    );
    let retry_attempts: i64 =
        query_scalar("select last_value from new_user_audit_generation_retry_calls")
            .fetch_one(&retry_pool)
            .await
            .expect("retry sequence must be readable");
    assert_eq!(
        retry_attempts, 4,
        "initial attempt plus three local retries"
    );

    query("drop trigger new_user_audit_generation_retry_timeout on new_user_audit_jobs")
        .execute(&retry_pool)
        .await
        .expect("retry trigger must be removed");
    query("drop function new_user_audit_generation_retry_timeout()")
        .execute(&retry_pool)
        .await
        .expect("retry trigger function must be removed");
    query("drop sequence new_user_audit_generation_retry_calls")
        .execute(&retry_pool)
        .await
        .expect("retry sequence must be removed");
}

async fn assert_unified_enqueue_and_finalizer_share_lock_order(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_095;
    let input = serde_json::json!({"schema_version": "lock-order-fixture"});

    query(
        "insert into telegram_new_user_profile_audits (chat_id, telegram_user_id, risk_score, risk_level, risk_signal_breakdown) values ($1, $2, 0, 'low', '[]'::jsonb)",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("lock-order audit fixture must be inserted");
    let job_id: i64 = query_scalar(
        "insert into new_user_audit_jobs (chat_id, telegram_user_id, snapshot_hash, prompt_version, input_json) values ($1, $2, 'lock-order-snapshot', 'prompt-v1', $3) returning id",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .bind(&input)
    .fetch_one(pool)
    .await
    .expect("lock-order job fixture must be inserted");

    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let enqueue_pool = pool.clone();
    let enqueue_barrier = Arc::clone(&barrier);
    let enqueue = async move {
        let mut tx = enqueue_pool.begin().await?;
        query("update new_user_audit_jobs set updated_at = now() where id = $1")
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
        enqueue_barrier.wait().await;
        query("select pg_sleep(0.1)").execute(&mut *tx).await?;
        query("update telegram_new_user_profile_audits set unified_audit_snapshot_hash = 'lock-order-snapshot' where chat_id = $1 and telegram_user_id = $2")
            .bind(CHAT_ID)
            .bind(USER_ID)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    };
    let finalizer_pool = pool.clone();
    let finalizer_barrier = Arc::clone(&barrier);
    let finalizer = async move {
        finalizer_barrier.wait().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let mut tx = finalizer_pool.begin().await?;
        query("update new_user_audit_jobs set status = 'succeeded' where id = $1")
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
        query("update telegram_new_user_profile_audits set risk_score = 80 where chat_id = $1 and telegram_user_id = $2")
            .bind(CHAT_ID)
            .bind(USER_ID)
            .execute(&mut *tx)
            .await?;
        query("insert into spam_review_requests (chat_id, telegram_user_id, risk_score, risk_signals) values ($1, $2, 80, '[]'::jsonb)")
            .bind(CHAT_ID)
            .bind(USER_ID)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    };

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::try_join!(enqueue, finalizer)
    })
    .await
    .expect("canonical lock order must not deadlock")
    .expect("concurrent enqueue and finalizer transactions must commit");

    query("delete from spam_review_requests where chat_id = $1 and telegram_user_id = $2")
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("lock-order review fixture must be removed");
}

async fn assert_successful_audit_replays_for_materialization(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_097;
    let input = serde_json::json!({"schema_version": "fixture-v1"});
    query(
        "insert into telegram_new_user_profile_audits (chat_id, telegram_user_id, risk_score, risk_level, risk_signal_breakdown) values ($1, $2, 0, 'low', '[]'::jsonb)",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("shadow replay audit baseline must be inserted");
    enqueue_new_user_audit_job(
        pool,
        NewUserAuditJobParams {
            chat_id: CHAT_ID,
            telegram_user_id: USER_ID,
            snapshot_hash: "shadow-replay-snapshot",
            prompt_version: "prompt-v1",
            input_json: &input,
            avatar_file_id: None,
            avatar_file_unique_id: None,
        },
    )
    .await
    .expect("shadow replay audit job must be enqueued");

    let shadow_claim = claim_next_new_user_audit_job(pool)
        .await
        .expect("shadow audit claim must execute")
        .expect("shadow audit job must be claimable");
    let assessment = serde_json::json!({
        "avatar_observation": null,
        "first_message_assessment": null,
        "profile_assessment": {
            "risk_patterns": ["no_material_risk_pattern"],
            "evidence": [], "contradictions": ["Нет признаков."],
            "review_priority": "low", "confidence": 0.5, "summary": "Нейтрально."
        }
    });
    assert_eq!(
        finalize_new_user_audit_job(
            pool,
            &shadow_claim,
            tg_ai_bot_teloxide::features::new_user_audit::repo::NewUserAuditOutcome {
                assessment_json: &assessment,
                provider: "fixture",
                model: "fixture",
            },
        )
        .await
        .expect("shadow audit success must finalize"),
        CasResult::Applied
    );
    query(
        "update new_user_audit_jobs set materialization_status = 'pending', materialization_lease_expires_at = null where id = $1",
    )
    .bind(shadow_claim.id)
    .execute(pool)
    .await
    .expect("stored replay fixture must be restored for materialization");

    let replay_claim = claim_next_new_user_audit_job(pool)
        .await
        .expect("replay claim must execute")
        .expect("successful assessment must be replayable");
    assert!(replay_claim.is_materialization_replay);
    assert_eq!(
        materialize_new_user_audit_job(
            pool,
            &replay_claim,
            &ScoreComponents {
                baseline_score: 0,
                baseline_signals: serde_json::json!([]),
                first_message_score: 0,
                first_message_signals: serde_json::json!([]),
                avatar_score: 0,
                avatar_signals: serde_json::json!([]),
            },
        )
        .await
        .expect("stored assessment materialization must finalize"),
        CasResult::Applied
    );
    let state: (String, String) =
        query_as("select status, materialization_status from new_user_audit_jobs where id = $1")
            .bind(replay_claim.id)
            .fetch_one(pool)
            .await
            .expect("materialized replay state must be stored");
    assert_eq!(state, ("succeeded".to_string(), "succeeded".to_string()));
}

async fn assert_audit_generation_is_durable_before_materialization(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_094;
    let input = serde_json::json!({"schema_version": "authoritative-boundary-fixture"});
    let assessment = serde_json::json!({
        "avatar_observation": null,
        "first_message_assessment": null,
        "profile_assessment": {
            "risk_patterns": ["no_material_risk_pattern"],
            "evidence": [], "contradictions": ["Нет признаков."],
            "review_priority": "low", "confidence": 0.5, "summary": "Нейтрально."
        }
    });
    enqueue_new_user_audit_job(
        pool,
        NewUserAuditJobParams {
            chat_id: CHAT_ID,
            telegram_user_id: USER_ID,
            snapshot_hash: "authoritative-boundary-snapshot",
            prompt_version: "prompt-v1",
            input_json: &input,
            avatar_file_id: None,
            avatar_file_unique_id: None,
        },
    )
    .await
    .expect("authoritative boundary job must be enqueued");
    let generation_claim = claim_next_new_user_audit_job(pool)
        .await
        .expect("authoritative generation claim must execute")
        .expect("authoritative generation job must be claimable");
    assert_eq!(
        finalize_new_user_audit_job(
            pool,
            &generation_claim,
            tg_ai_bot_teloxide::features::new_user_audit::repo::NewUserAuditOutcome {
                assessment_json: &assessment,
                provider: "fixture",
                model: "fixture",
            },
        )
        .await
        .expect("authoritative generation must persist before materialization"),
        CasResult::Applied
    );
    let generation_state: (String, serde_json::Value, String, i32, bool) = query_as(
        "select status, assessment_json, materialization_status, materialization_attempts, lease_expires_at is null from new_user_audit_jobs where id = $1",
    )
    .bind(generation_claim.id)
    .fetch_one(pool)
    .await
    .expect("durable authoritative generation state must be stored");
    assert_eq!(
        generation_state,
        (
            "succeeded".into(),
            assessment.clone(),
            "pending".into(),
            0,
            true
        )
    );

    query("update new_user_audit_jobs set materialization_status = 'stale' where id <> $1 and assessment_json is not null")
        .bind(generation_claim.id)
        .execute(pool)
        .await
        .expect("other replay fixtures must not affect authoritative boundary assertion");

    let materialization_claim = claim_next_new_user_audit_job(pool)
        .await
        .expect("materialization replay claim must execute")
        .expect("stored authoritative assessment must be replayable");
    assert_eq!(materialization_claim.id, generation_claim.id);
    assert!(materialization_claim.is_materialization_replay);
    assert_eq!(materialization_claim.attempts, generation_claim.attempts);
    assert_eq!(
        mark_new_user_audit_materialization_retry(pool, &materialization_claim, "sql_transient",)
            .await
            .expect("transient materialization failure must schedule replay"),
        CasResult::Applied
    );
    assert!(
        claim_next_new_user_audit_job(pool)
            .await
            .expect("generation queue check must execute")
            .is_none(),
        "a materialization retry must never reopen authoritative LLM generation"
    );
}

async fn assert_new_user_audit_generation_materialization_upgrade(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const PROCESSING_USER_ID: i64 = 9_000_092;
    const RETRY_USER_ID: i64 = 9_000_093;
    let assessment = serde_json::json!({"legacy": "assessment"});
    for (user_id, status) in [
        (PROCESSING_USER_ID, "processing"),
        (RETRY_USER_ID, "retry_wait"),
    ] {
        query(
            "insert into new_user_audit_jobs (chat_id, telegram_user_id, snapshot_hash, prompt_version, input_json, status, attempts, next_attempt_at, assessment_json, error_kind) values ($1, $2, $3, 'prompt-v1', '{}'::jsonb, $4, 1, now() + interval '10 minutes', $5, 'legacy_error')",
        )
        .bind(CHAT_ID)
        .bind(user_id)
        .bind(format!("generation-materialization-upgrade-{user_id}"))
        .bind(status)
        .bind(&assessment)
        .execute(pool)
        .await
        .expect("legacy assessment fixture must be inserted");
    }

    sqlx::raw_sql(include_str!(
        "../migrations/20260730122000_new_user_audit_generation_materialization_boundary.sql"
    ))
    .execute(pool)
    .await
    .expect("generation/materialization boundary migration must normalize legacy rows");

    let rows: Vec<(i64, String, String, bool, Option<String>)> = query_as(
        "select telegram_user_id, status, materialization_status, materialization_next_attempt_at > now(), materialization_error_kind from new_user_audit_jobs where chat_id = $1 and telegram_user_id = any($2) order by telegram_user_id",
    )
    .bind(CHAT_ID)
    .bind([PROCESSING_USER_ID, RETRY_USER_ID])
    .fetch_all(pool)
    .await
    .expect("normalized legacy assessment rows must be queryable");
    assert_eq!(
        rows,
        vec![
            (
                PROCESSING_USER_ID,
                "succeeded".into(),
                "pending".into(),
                false,
                None
            ),
            (
                RETRY_USER_ID,
                "succeeded".into(),
                "retry_wait".into(),
                true,
                Some("legacy_error".into())
            ),
        ]
    );
}

async fn assert_new_user_audit_materialization_lifecycle(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_096;
    let input = serde_json::json!({"schema_version": "fixture-v1"});
    query(
        "insert into telegram_new_user_profile_audits (chat_id, telegram_user_id, risk_score, risk_level, risk_signal_breakdown) values ($1, $2, 0, 'low', '[]'::jsonb)",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("materialization lifecycle baseline must be inserted");
    enqueue_new_user_audit_job(
        pool,
        NewUserAuditJobParams {
            chat_id: CHAT_ID,
            telegram_user_id: USER_ID,
            snapshot_hash: "materialization-lifecycle-snapshot",
            prompt_version: "prompt-v1",
            input_json: &input,
            avatar_file_id: None,
            avatar_file_unique_id: None,
        },
    )
    .await
    .expect("materialization lifecycle job must be enqueued");
    let generation_claim = claim_next_new_user_audit_job(pool)
        .await
        .expect("generation claim must execute")
        .expect("generation job must be claimable");
    let assessment = serde_json::json!({
        "avatar_observation": null,
        "first_message_assessment": null,
        "profile_assessment": {
            "risk_patterns": ["no_material_risk_pattern"],
            "evidence": [], "contradictions": ["Нет признаков."],
            "review_priority": "low", "confidence": 0.5, "summary": "Нейтрально."
        }
    });
    assert_eq!(
        finalize_new_user_audit_job(
            pool,
            &generation_claim,
            tg_ai_bot_teloxide::features::new_user_audit::repo::NewUserAuditOutcome {
                assessment_json: &assessment,
                provider: "fixture",
                model: "fixture",
            },
        )
        .await
        .expect("generation must succeed before replay"),
        CasResult::Applied
    );

    let first_replay = claim_next_new_user_audit_job(pool)
        .await
        .expect("first replay claim must execute")
        .expect("successful generation must be replayable");
    assert!(first_replay.is_materialization_replay);
    assert_eq!(first_replay.attempts, generation_claim.attempts);
    assert_eq!(first_replay.materialization_attempts, 1);
    assert_eq!(
        mark_new_user_audit_materialization_retry(pool, &first_replay, "sql_transient")
            .await
            .expect("replay retry finalizer must execute"),
        CasResult::Applied
    );
    let retry_state: (String, String, i32, bool, bool) = query_as(
        "select status, materialization_status, materialization_attempts, materialization_processing_started_at is null, materialization_lease_expires_at is null from new_user_audit_jobs where id = $1",
    )
    .bind(first_replay.id)
    .fetch_one(pool)
    .await
    .expect("replay retry state must be stored");
    assert_eq!(
        retry_state,
        ("succeeded".into(), "retry_wait".into(), 1, true, true)
    );

    query("update new_user_audit_jobs set materialization_next_attempt_at = now() - interval '1 second' where id = $1")
        .bind(first_replay.id)
        .execute(pool)
        .await
        .expect("replay retry must be made due");
    let stale_replay = claim_next_new_user_audit_job(pool)
        .await
        .expect("retry replay claim must execute")
        .expect("retry replay must be claimable");
    query("update new_user_audit_jobs set materialization_lease_expires_at = now() - interval '1 second' where id = $1")
        .bind(stale_replay.id)
        .execute(pool)
        .await
        .expect("replay lease must expire");
    let current_replay = claim_next_new_user_audit_job(pool)
        .await
        .expect("expired replay claim must execute")
        .expect("expired replay must be reclaimed");
    assert_eq!(
        current_replay.materialization_attempts,
        stale_replay.materialization_attempts + 1
    );
    assert_eq!(
        mark_new_user_audit_materialization_stale(pool, &stale_replay, "malformed_assessment")
            .await
            .expect("stale replay finalizer must execute"),
        CasResult::LeaseLost
    );
    assert_eq!(
        mark_new_user_audit_materialization_stale(pool, &current_replay, "malformed_assessment")
            .await
            .expect("current replay stale finalizer must execute"),
        CasResult::Applied
    );
    let terminal_state: (String, String, String) = query_as(
        "select status, materialization_status, materialization_error_kind from new_user_audit_jobs where id = $1",
    )
    .bind(current_replay.id)
    .fetch_one(pool)
    .await
    .expect("stale replay state must be stored");
    assert_eq!(
        terminal_state,
        (
            "succeeded".into(),
            "stale".into(),
            "malformed_assessment".into()
        )
    );

    query("update new_user_audit_jobs set materialization_status = 'processing', materialization_attempts = 6, materialization_lease_expires_at = now() + interval '10 minutes' where id = $1")
        .bind(current_replay.id)
        .execute(pool)
        .await
        .expect("materialization exhaustion fixture must be stored");
    let exhausted_replay = tg_ai_bot_teloxide::features::new_user_audit::repo::NewUserAuditJob {
        materialization_attempts: 6,
        ..current_replay.clone()
    };
    assert_eq!(
        mark_new_user_audit_materialization_retry(pool, &exhausted_replay, "sql_transient")
            .await
            .expect("exhausted replay finalizer must execute"),
        CasResult::Applied
    );
    let exhaustion_state: (String, String, String) = query_as(
        "select status, materialization_status, materialization_error_kind from new_user_audit_jobs where id = $1",
    )
    .bind(current_replay.id)
    .fetch_one(pool)
    .await
    .expect("exhausted replay state must be stored");
    assert_eq!(
        exhaustion_state,
        ("succeeded".into(), "stale".into(), "retry_exhausted".into())
    );

    query("update new_user_audit_jobs set materialization_status = 'pending', materialization_version = 'obsolete-version' where id = $1")
        .bind(current_replay.id)
        .execute(pool)
        .await
        .expect("obsolete replay version fixture must be stored");
    assert!(
        claim_next_new_user_audit_job(pool)
            .await
            .expect("version-gated replay claim must execute")
            .is_none(),
        "authoritative replay must require the current materialization version"
    );
}

async fn assert_review_delivery_finalization_requires_current_claim(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_002;
    query("insert into telegram_chat_users (chat_id, telegram_user_id) values ($1, $2)")
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("review CAS chat user must exist");
    query("insert into telegram_user_profiles (telegram_user_id, first_name) values ($1, 'Review CAS')")
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("review CAS profile must exist");
    query("insert into telegram_new_user_profile_audits (chat_id, telegram_user_id, risk_score, risk_level, risk_signal_breakdown) values ($1, $2, 80, 'high', '[]'::jsonb)")
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("high-risk audit must exist");
    let first_claim = create_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("review creation must succeed")
        .expect("high-risk review must be claimed");
    query("update spam_review_requests set notification_lease_expires_at = now() - interval '1 second' where id = $1")
        .bind(first_claim.id)
        .execute(pool)
        .await
        .expect("review lease must expire");
    let second_claim = claim_next_review_delivery(pool)
        .await
        .expect("reclaimed review must be claimable")
        .expect("review must be reclaimed");
    assert!(second_claim.notification_attempts > first_claim.notification_attempts);
    assert_eq!(
        mark_review_delivery_succeeded(pool, &first_claim, 1001)
            .await
            .expect("stale review finalization must execute"),
        tg_ai_bot_teloxide::features::jobs::claim::CasResult::LeaseLost
    );
    assert_eq!(
        mark_review_delivery_succeeded(pool, &second_claim, 1002)
            .await
            .expect("current review finalization must execute"),
        tg_ai_bot_teloxide::features::jobs::claim::CasResult::Applied
    );
    let state: (String, i32, Option<i32>) = query_as(
        "select notification_status, notification_attempts, notification_message_id from spam_review_requests where id = $1",
    )
    .bind(second_claim.id)
    .fetch_one(pool)
    .await
    .expect("finalized review must be stored");
    assert_eq!(
        state,
        (
            "sent".into(),
            second_claim.notification_attempts,
            Some(1002)
        )
    );
}

async fn assert_review_delivery_payload_cas_blocks_replaced_and_lowered_risk(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_006;
    query("insert into telegram_chat_users (chat_id, telegram_user_id) values ($1, $2)")
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("review payload CAS chat user must exist");
    query("insert into telegram_user_profiles (telegram_user_id, first_name) values ($1, 'Payload CAS')")
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("review payload CAS profile must exist");
    query("insert into telegram_new_user_profile_audits (chat_id, telegram_user_id, risk_score, risk_level, risk_signal_breakdown) values ($1, $2, 80, 'high', '[{\"label\": \"single_message_account\"}]'::jsonb)")
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("high-risk payload CAS audit must exist");

    let replaced_claim = create_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("payload CAS review creation must succeed")
        .expect("high-risk payload CAS review must be claimed");
    query("update spam_review_requests set risk_signals = '[{\"label\": \"personal_channel_attached\"}]'::jsonb where id = $1")
        .bind(replaced_claim.id)
        .execute(pool)
        .await
        .expect("review payload must be replaceable while claimed");
    let listener = TcpListener::bind("127.0.0.1:0").expect("Telegram stub listener must bind");
    listener
        .set_nonblocking(true)
        .expect("Telegram stub listener must be nonblocking");
    let bot = Bot::new("test-token").set_api_url(
        format!(
            "http://{}/",
            listener
                .local_addr()
                .expect("Telegram stub address must exist")
        )
        .parse()
        .expect("Telegram stub API URL must parse"),
    );
    send_review(&bot, pool, &replaced_claim)
        .await
        .expect("replaced payload must be skipped before Telegram delivery");
    assert_eq!(
        listener
            .accept()
            .expect_err("replaced payload must not open a Telegram connection")
            .kind(),
        ErrorKind::WouldBlock
    );

    query("update spam_review_requests set notification_status = 'pending', notification_lease_expires_at = null where id = $1")
        .bind(replaced_claim.id)
        .execute(pool)
        .await
        .expect("replaced payload fixture must be returned to pending");
    let lowered_claim = claim_next_review_delivery(pool)
        .await
        .expect("replaced high-risk review must be reclaimable")
        .expect("replaced high-risk review must be claimed again");
    query("update spam_review_requests set risk_score = 69 where id = $1")
        .bind(lowered_claim.id)
        .execute(pool)
        .await
        .expect("review risk must be lowerable while claimed");
    send_review(&bot, pool, &lowered_claim)
        .await
        .expect("lowered-risk payload must be skipped before Telegram delivery");
    assert_eq!(
        listener
            .accept()
            .expect_err("lowered-risk payload must not open a Telegram connection")
            .kind(),
        ErrorKind::WouldBlock
    );
}

async fn assert_stale_review_delivery_failure_does_not_finalize_replaced_payload(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_007;
    query("insert into telegram_chat_users (chat_id, telegram_user_id) values ($1, $2)")
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("stale failure chat user must exist");
    query("insert into telegram_user_profiles (telegram_user_id, first_name) values ($1, 'Stale failure')")
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("stale failure profile must exist");
    query("insert into telegram_new_user_profile_audits (chat_id, telegram_user_id, risk_score, risk_level, risk_signal_breakdown) values ($1, $2, 80, 'high', '[{\"label\": \"single_message_account\"}]'::jsonb)")
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("stale failure audit must exist");
    let review = create_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("stale failure review creation must succeed")
        .expect("stale failure review must be claimed");
    let review_id = review.id;

    let listener = TcpListener::bind("127.0.0.1:0").expect("stale failure listener must bind");
    let address = listener
        .local_addr()
        .expect("stale failure listener must have an address");
    let (request_received, request_received_wait) = sync_channel(1);
    let (send_response, send_response_wait) = sync_channel(1);
    let response_task = std::thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .expect("stale failure listener must accept the request");
        let mut request = [0; 1_024];
        let _ = stream.read(&mut request);
        request_received
            .send(())
            .expect("stale failure request receipt must be reported");
        send_response_wait
            .recv()
            .expect("stale failure response must be released");
        stream
            .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("stale failure response must be written");
    });
    let bot = Bot::new("test-token").set_api_url(
        format!("http://{address}/")
            .parse()
            .expect("stale failure Telegram API URL must parse"),
    );
    let delivery_pool = pool.clone();
    let delivery = tokio::spawn(async move { send_review(&bot, &delivery_pool, &review).await });
    tokio::task::spawn_blocking(move || {
        request_received_wait
            .recv()
            .expect("stale failure request must reach Telegram")
    })
    .await
    .expect("stale failure request wait must not panic");

    query("update spam_review_requests set risk_signals = '[{\"label\": \"personal_channel_attached\"}]'::jsonb where id = $1")
        .bind(review_id)
        .execute(pool)
        .await
        .expect("review payload must change while the HTTP request is in flight");
    send_response
        .send(())
        .expect("stale failure response must be released");
    assert!(
        delivery
            .await
            .expect("stale failure delivery task must not panic")
            .is_err(),
        "HTTP 500 must reach the failure finalizer"
    );
    response_task
        .join()
        .expect("stale failure response task must complete");

    let state: (String, i32, Option<String>, serde_json::Value) = query_as(
        "select notification_status, notification_consecutive_failures, notification_error_kind, risk_signals from spam_review_requests where id = $1",
    )
    .bind(review_id)
    .fetch_one(pool)
    .await
    .expect("stale failure state must be readable");
    assert_eq!(state.0, "processing");
    assert_eq!(state.1, 0);
    assert_eq!(state.2, None);
    assert_eq!(
        state.3,
        serde_json::json!([{"label": "personal_channel_attached"}])
    );
}

async fn assert_review_delivery_retry_uses_consecutive_failures(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_004;

    query("insert into telegram_chat_users (chat_id, telegram_user_id) values ($1, $2)")
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("review retry chat user must exist");
    query("insert into telegram_user_profiles (telegram_user_id, first_name) values ($1, 'Review retry')")
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("review retry profile must exist");
    query("insert into telegram_new_user_profile_audits (chat_id, telegram_user_id, risk_score, risk_level, risk_signal_breakdown) values ($1, $2, 80, 'high', '[]'::jsonb)")
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("high-risk retry audit must exist");

    let initial_claim = create_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("review creation must succeed")
        .expect("high-risk review must be claimed");
    query(
        "update spam_review_requests set notification_status = 'retry_wait', notification_attempts = 20, notification_consecutive_failures = 0, notification_next_attempt_at = now(), notification_processing_started_at = null, notification_lease_expires_at = null where id = $1",
    )
    .bind(initial_claim.id)
    .execute(pool)
    .await
    .expect("high claim sequence fixture must be stored");

    let retry_claim = create_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("specific retry claim must succeed")
        .expect("retryable high-risk review must be claimed");
    assert_eq!(retry_claim.notification_attempts, 21);
    assert_eq!(retry_claim.notification_consecutive_failures, 0);

    let listener =
        TcpListener::bind("127.0.0.1:0").expect("local transient failure listener must bind");
    let address = listener
        .local_addr()
        .expect("local transient failure listener must have an address");
    let response_task = std::thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .expect("local transient failure listener must accept the request");
        let mut request = [0; 1_024];
        let _ = stream.read(&mut request);
        stream
            .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("local transient failure response must be written");
    });
    let bot = Bot::new("test-token").set_api_url(
        format!("http://{address}/")
            .parse()
            .expect("local Telegram API URL must parse"),
    );
    assert!(
        send_review(&bot, pool, &retry_claim).await.is_err(),
        "HTTP 500 must exercise production transient failure handling"
    );
    response_task
        .join()
        .expect("local transient failure response task must complete");

    let failed_state: (String, i32, i32) = query_as(
        "select notification_status, notification_attempts, notification_consecutive_failures from spam_review_requests where id = $1",
    )
    .bind(retry_claim.id)
    .fetch_one(pool)
    .await
    .expect("transient failure state must be stored");
    assert_eq!(failed_state, ("retry_wait".into(), 21, 1));

    query("update spam_review_requests set notification_next_attempt_at = now() where id = $1")
        .bind(retry_claim.id)
        .execute(pool)
        .await
        .expect("retry must be made due");
    let success_claim = claim_next_review_delivery(pool)
        .await
        .expect("retry claim query must succeed")
        .expect("failed review must be claimable again");
    assert_eq!(success_claim.id, retry_claim.id);
    assert_eq!(success_claim.notification_attempts, 22);
    assert_eq!(success_claim.notification_consecutive_failures, 1);
    assert_eq!(
        mark_review_delivery_succeeded(pool, &success_claim, 1_003)
            .await
            .expect("production success finalizer must execute"),
        CasResult::Applied
    );
    let success_state: (String, i32) = query_as(
        "select notification_status, notification_consecutive_failures from spam_review_requests where id = $1",
    )
    .bind(success_claim.id)
    .fetch_one(pool)
    .await
    .expect("successful retry state must be stored");
    assert_eq!(success_state, ("sent".into(), 0));
}

async fn assert_terminal_review_delivery_stays_closed(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 9_000_003;
    query("insert into telegram_chat_users (chat_id, telegram_user_id) values ($1, $2)")
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("terminal review chat user must exist");
    query("insert into telegram_user_profiles (telegram_user_id, first_name) values ($1, 'Terminal review')")
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("terminal review profile must exist");
    query("insert into telegram_new_user_profile_audits (chat_id, telegram_user_id, risk_score, risk_level, risk_signal_breakdown) values ($1, $2, 80, 'high', '[]'::jsonb)")
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("terminal high-risk audit must exist");
    let review = create_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("terminal review creation must succeed")
        .expect("high-risk review must be claimed");
    query("update spam_review_requests set notification_status = 'failed', notification_consecutive_failures = 5, notified_risk_score = null, notified_risk_signals = null where id = $1")
        .bind(review.id)
        .execute(pool)
        .await
        .expect("terminal delivery state must be writable");
    query("update telegram_new_user_profile_audits set risk_score = 90, risk_signal_breakdown = '[{\"label\": \"late_signal\"}]'::jsonb where chat_id = $1 and telegram_user_id = $2")
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("late audit signal must be writable");
    assert!(
        create_review(pool, CHAT_ID, USER_ID)
            .await
            .expect("terminal review refresh must succeed")
            .is_none(),
        "terminal delivery must not be reopened by enrichment"
    );
    let state: (String, i32) = query_as(
        "select notification_status, notification_consecutive_failures from spam_review_requests where id = $1",
    )
    .bind(review.id)
    .fetch_one(pool)
    .await
    .expect("terminal delivery state must remain stored");
    assert_eq!(state, ("failed".into(), 5));
}

async fn assert_clean_database_migrations(pool: &PgPool) {
    let migration_count: i64 = query_scalar("select count(*) from _sqlx_migrations")
        .fetch_one(pool)
        .await
        .expect("migration ledger must exist");
    assert!(migration_count > 0, "all migrations must be applied");

    let post_comment_jobs: Option<String> =
        query_scalar("select to_regclass('public.post_comment_jobs')::text")
            .fetch_one(pool)
            .await
            .expect("post_comment_jobs lookup must succeed");
    assert_eq!(post_comment_jobs.as_deref(), Some("post_comment_jobs"));

    let sent_at_column: bool = query_scalar(
        r#"
        select exists (
            select 1 from information_schema.columns
            where table_schema = 'public' and table_name = 'post_comment_jobs' and column_name = 'sent_at'
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .expect("sent_at column lookup must succeed");
    assert!(
        sent_at_column,
        "sent comment timestamp migration must be applied"
    );

    let public_messages_view: Option<String> =
        query_scalar("select to_regclass('mcp_public.telegram_messages')::text")
            .fetch_one(pool)
            .await
            .expect("public MCP view lookup must succeed");
    assert_eq!(
        public_messages_view.as_deref(),
        Some("mcp_public.telegram_messages")
    );
}

async fn assert_sent_comment_requires_sent_at(pool: &PgPool) {
    let error = query(
        r#"
        insert into post_comment_jobs
            (discussion_chat_id, discussion_message_id, source_channel_id, source_message_id,
             cleaned_post_text, status)
        values
            (-1001932061163, 990001, -1001575496091, 990001, 'invalid sent comment', 'sent')
        "#,
    )
    .execute(pool)
    .await
    .expect_err("sent comment without sent_at must violate the database constraint");

    assert!(
        error.as_database_error().is_some_and(|database_error| {
            database_error.constraint() == Some("post_comment_jobs_sent_requires_sent_at")
        }),
        "sent comment must fail the sent_at constraint: {error}"
    );
}

async fn assert_public_mcp_scope(pool: &PgPool) {
    query(
        r#"
        insert into telegram_messages
            (chat_id, message_id, source_channel_id, source_message_id, is_automatic_forward, text)
        values
            (-1001932061163, 100, -1001575496091, 100, true, 'discussion message'),
            (123456789, 100, -1001575496091, 100, true, 'private forward')
        "#,
    )
    .execute(pool)
    .await
    .expect("test messages must be inserted");

    let public_messages: Vec<String> = query_scalar(
        r#"
        select text
        from mcp_public.telegram_messages
        where source_channel_id = -1001575496091 and source_message_id = 100
        order by chat_id
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("public MCP view query must succeed");

    assert_eq!(public_messages, vec!["discussion message"]);
}

async fn assert_stats_renderers_share_period_data(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    let window = ReportWindow::new(
        Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0)
            .single()
            .expect("test start timestamp must be valid"),
        Utc.with_ymd_and_hms(2000, 1, 2, 0, 0, 0)
            .single()
            .expect("test end timestamp must be valid"),
    );
    let inside_at = Utc
        .with_ymd_and_hms(2000, 1, 1, 12, 0, 0)
        .single()
        .expect("test inside timestamp must be valid");
    let comment_at = window.end_at - Duration::minutes(5);
    let job_created_at = comment_at - Duration::minutes(1);
    let message_before_comment_was_sent = comment_at - Duration::seconds(30);
    let reply_after_window = window.end_at + Duration::minutes(1);
    query(
        r#"
        insert into telegram_messages
            (chat_id, message_id, user_id, source_channel_id, reply_to_message_id, text, created_at)
        values
            ($1, 901, 901, null, null, 'inside report window', $2),
            ($1, 902, 902, -1001575496091, null, 'outside report window', $3),
            ($1, 903, 903, null, 1911, 'cohort reply after report window', $4),
            ($1, 904, 904, null, null, 'message before comment was sent', $5)
        "#,
    )
    .bind(CHAT_ID)
    .bind(inside_at)
    .bind(window.end_at)
    .bind(reply_after_window)
    .bind(message_before_comment_was_sent)
    .execute(pool)
    .await
    .expect("windowed stats messages must be inserted");
    query(
        r#"
        insert into post_comment_jobs
            (discussion_chat_id, discussion_message_id, source_channel_id, source_message_id,
             cleaned_post_text, status, bot_comment_message_id, created_at, sent_at)
        values
            ($1, 911, -1001575496091, 911, 'inside comment', 'sent', 1911, $4, $2),
            ($1, 912, -1001575496091, 912, 'not yet sent at report end', 'sent', 1912, $4, $3)
        "#,
    )
    .bind(CHAT_ID)
    .bind(comment_at)
    .bind(window.end_at)
    .bind(job_created_at)
    .execute(pool)
    .await
    .expect("windowed comment jobs must be inserted");

    let summary = stats_repo::chat_stats_summary(pool, CHAT_ID, window)
        .await
        .expect("period summary query must succeed");
    let attraction = stats_repo::chat_attraction_metrics(pool, CHAT_ID, window)
        .await
        .expect("attraction query must succeed");
    let top_users = stats_repo::period_top_users(pool, CHAT_ID, window, 10)
        .await
        .expect("period top users query must succeed");
    let bot_comments = stats_repo::bot_comments_for_period(pool, CHAT_ID, window, 10)
        .await
        .expect("period bot comments query must succeed");
    assert_eq!(summary.messages, 2, "summary must use the fixed window");
    assert_eq!(
        summary.bot_comments, 1,
        "summary must include only comments sent inside the fixed window"
    );
    assert_eq!(top_users.len(), 2, "top users must use the fixed window");
    assert_eq!(
        bot_comments.len(),
        1,
        "bot comments must use the fixed window"
    );
    assert_eq!(bot_comments[0].source_message_id, 911);
    assert_eq!(
        attraction.messages_5m, "0.00",
        "the mature five-minute cohort must start when the comment was sent"
    );
    assert_eq!(
        attraction.messages_30m, "-",
        "an incomplete 30-minute cohort must not be reported as zero"
    );
    assert_eq!(
        attraction.messages_24h, "-",
        "an incomplete 24-hour cohort must not be reported as zero"
    );
    assert_eq!(
        attraction.users_30m, "-",
        "an incomplete 30-minute user cohort must be unavailable"
    );
    assert_eq!(
        bot_comments[0].messages_30m, 1,
        "per-comment engagement must exclude activity before sent_at"
    );
    let data = ChatStatsReportData {
        period: StatsPeriod::Day,
        summary: summary.clone(),
        attraction: AttractionMetrics {
            messages_5m: attraction.messages_5m,
            messages_30m: attraction.messages_30m,
            messages_24h: attraction.messages_24h,
            users_30m: attraction.users_30m,
        },
        top_users: Vec::new(),
        bot_comments: Vec::new(),
    };

    let time = TimeContext::from_name("Europe/Moscow").expect("test time zone must be valid");
    let html = render_html::chat_stats(&data, &time);
    let rich = render_rich::chat_stats(&data, CHAT_ID, &time);
    let messages = format!("{}", summary.messages);
    let active_users = format!("{}", summary.active_users);
    assert!(html.contains(&messages));
    assert!(rich.contains(&messages));
    assert!(html.contains(&active_users));
    assert!(rich.contains(&active_users));
}

async fn assert_feature_gated_jobs(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 1001;
    const MESSAGE_ID: i32 = 300;

    query("insert into telegram_user_profiles (telegram_user_id, first_name) values ($1, 'Gate')")
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("embedding author profile must be inserted");
    query(
        "insert into telegram_messages (chat_id, message_id, user_id, text) values ($1, $2, $3, 'обычное сообщение')",
    )
    .bind(CHAT_ID)
    .bind(MESSAGE_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("embedding source message must be inserted");

    enqueue_message_embedding_if_enabled(pool, false, CHAT_ID, MESSAGE_ID)
        .await
        .expect("disabled embedding gate must succeed");
    let embedding_jobs: i64 = query_scalar(
        "select count(*) from telegram_message_embeddings where chat_id = $1 and message_id = $2",
    )
    .bind(CHAT_ID)
    .bind(MESSAGE_ID)
    .fetch_one(pool)
    .await
    .expect("embedding job count query must succeed");
    assert_eq!(embedding_jobs, 0);

    enqueue_message_embedding_if_enabled(pool, true, CHAT_ID, MESSAGE_ID)
        .await
        .expect("enabled embedding gate must succeed");
    let embedding_jobs: i64 = query_scalar(
        "select count(*) from telegram_message_embeddings where chat_id = $1 and message_id = $2",
    )
    .bind(CHAT_ID)
    .bind(MESSAGE_ID)
    .fetch_one(pool)
    .await
    .expect("enabled embedding job count query must succeed");
    assert_eq!(embedding_jobs, 1);
}

async fn assert_agent_note_contract(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 2001;
    const AUTHOR_ID: i64 = 3001;

    query("insert into telegram_user_profiles (telegram_user_id, first_name) values ($1, 'Notes')")
        .bind(USER_ID)
        .execute(pool)
        .await
        .expect("note target profile must be inserted");

    let missing_sources = add_user_note_from_search(pool, CHAT_ID, USER_ID, AUTHOR_ID, "факт", &[])
        .await
        .expect_err("automatic note without source messages must fail");
    assert!(
        missing_sources
            .to_string()
            .contains("automatic user notes require message sources")
    );

    add_user_note_from_search(
        pool,
        CHAT_ID,
        USER_ID,
        AUTHOR_ID,
        "  важный\nфакт ",
        &[401, 402],
    )
    .await
    .expect("sourced note must be inserted");
    add_user_note_from_search(pool, CHAT_ID, USER_ID, 9999, "важный факт", &[403])
        .await
        .expect("identical sourced note must deduplicate");

    let (note, created_by_user_id, source_message_ids): (String, i64, serde_json::Value) = query_as(
        "select note, created_by_user_id, source_message_ids from telegram_user_notes where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("deduplicated note must exist");
    assert_eq!(note, "важный факт");
    assert_eq!(created_by_user_id, AUTHOR_ID);
    assert_eq!(source_message_ids, serde_json::json!([401, 402]));

    let note_count: i64 = query_scalar(
        "select count(*) from telegram_user_notes where chat_id = $1 and telegram_user_id = $2 and status = 'active'",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("note count query must succeed");
    assert_eq!(note_count, 1);
}

async fn assert_review_deduplication(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const USER_ID: i64 = 42;

    query(
        "insert into telegram_chat_users (chat_id, telegram_user_id, first_message_id) values ($1, $2, 200)",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("chat user must be inserted");
    query(
        "insert into telegram_user_profiles (telegram_user_id, first_name, profile_photo_file_unique_id) values ($1, 'Тест', 'photo-42')",
    )
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("user profile must be inserted");
    query(
        r#"
        insert into telegram_new_user_profile_audits
            (
                chat_id, telegram_user_id, risk_baseline_score,
                risk_baseline_signals, risk_score, risk_level, risk_signal_breakdown
            )
        values ($1, $2, 65, '[]'::jsonb, 65, 'medium', '[]'::jsonb)
        "#,
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("medium-risk audit must be inserted");

    let first_review = create_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("review creation must succeed");
    assert!(
        first_review.is_none(),
        "medium-risk audit must be recorded without queuing a Telegram card"
    );
    let review_count: i64 = query_scalar(
        "select count(*) from spam_review_requests where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("medium-risk review record must be queryable");
    assert_eq!(review_count, 1);

    query(
        "update telegram_new_user_profile_audits set risk_score = 73, risk_level = 'high', risk_signal_breakdown = '[{\"label\": \"unified_signal\"}]'::jsonb where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("unified risk snapshot must be applied");
    let updated_review = create_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("unified risk must refresh review delivery")
        .expect("high-risk review must be claimed for its first delivery");
    assert_eq!(updated_review.notification_message_id, None);

    let (risk_score, risk_level): (i32, String) = query_as(
        "select risk_score, risk_level from telegram_new_user_profile_audits where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
        .expect("updated unified risk audit must exist");
    assert_eq!(risk_score, 73);
    assert_eq!(risk_level, "high");

    let (review_score, review_signals): (i32, serde_json::Value) = query_as(
        "select risk_score, risk_signals from spam_review_requests where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
        .expect("review snapshot must be refreshed after a unified risk signal");
    assert_eq!(review_score, 73);
    assert!(
        review_signals
            .as_array()
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("label").and_then(serde_json::Value::as_str) == Some("unified_signal")
            })),
        "review snapshot must include the later unified signal: {review_signals}"
    );

    let (notification_status, notification_attempts, notification_message_id): (String, i32, Option<i32>) = query_as(
        "select notification_status, notification_attempts, notification_message_id from spam_review_requests where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("high-risk review must be queued for its first delivery");
    assert_eq!(notification_status, "processing");
    assert_eq!(notification_attempts, 1);
    assert_eq!(notification_message_id, None);

    query(
        "update spam_review_requests set notification_status = 'retry_wait', notification_next_attempt_at = now(), notification_lease_expires_at = null where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("failed review edit must remain retryable");
    let retried_review = create_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("retryable review delivery claim must succeed");
    assert!(
        retried_review.is_some(),
        "a pending failed notification must be claimable again"
    );
    let (notification_status, notification_attempts, notification_message_id): (String, i32, Option<i32>) = query_as(
        "select notification_status, notification_attempts, notification_message_id from spam_review_requests where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("review delivery lifecycle must be persisted");
    assert_eq!(notification_status, "processing");
    assert_eq!(notification_attempts, 2);
    assert_eq!(notification_message_id, None);

    query(
        "update spam_review_requests set status = 'confirmed_not_spam', notification_status = 'sent' where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("review must be confirmable");
    query(
        "update telegram_new_user_profile_audits set risk_score = 90, risk_signal_breakdown = '[{\"label\": \"later_signal\"}]'::jsonb where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("later audit snapshot must be writable");
    let confirmed_review = create_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("confirmed review snapshot refresh must succeed");
    assert!(confirmed_review.is_none());
    let (confirmed_status, confirmed_notification_status, confirmed_score): (String, String, i32) =
        query_as(
            "select status, notification_status, risk_score from spam_review_requests where chat_id = $1 and telegram_user_id = $2",
        )
        .bind(CHAT_ID)
        .bind(USER_ID)
        .fetch_one(pool)
        .await
        .expect("confirmed review must remain stored");
    assert_eq!(confirmed_status, "confirmed_not_spam");
    assert_eq!(confirmed_notification_status, "sent");
    assert_eq!(confirmed_score, 90);

    let review_count: i64 = query_scalar(
        "select count(*) from spam_review_requests where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("review count query must succeed");
    assert_eq!(review_count, 1);
}

async fn assert_comment_job_lifecycle(pool: &PgPool) {
    let retry_job = claim_created_job(pool, 1).await;
    assert_eq!(
        mark_post_comment_pre_send_failed(pool, &retry_job, CommentErrorKind::Transient)
            .await
            .expect("pre-send failure must execute"),
        CasResult::Applied
    );
    assert_job_state(pool, retry_job.id, "retry_wait", Some("transient"), true).await;

    make_ready_for_retry(pool, retry_job.id).await;
    let retry_job = claim_next_post_comment_job(pool)
        .await
        .expect("second claim must succeed")
        .expect("retry job must be claimable");
    assert_eq!(
        mark_post_comment_pre_send_failed(pool, &retry_job, CommentErrorKind::RateLimited)
            .await
            .expect("rate limit failure must execute"),
        CasResult::Applied
    );
    assert_job_state(pool, retry_job.id, "retry_wait", Some("rate_limited"), true).await;

    let stale_job = claim_created_job(pool, 2).await;
    query(
        "update post_comment_jobs set lease_expires_at = now() - interval '1 second' where id = $1",
    )
    .bind(stale_job.id)
    .execute(pool)
    .await
    .expect("processing lease must expire");
    let current_job = claim_next_post_comment_job(pool)
        .await
        .expect("reclaim must succeed")
        .expect("expired processing job must be reclaimed");
    assert_eq!(current_job.id, stale_job.id);
    let reclaim_count: i32 =
        query_scalar("select lease_reclaim_count from post_comment_jobs where id = $1")
            .bind(current_job.id)
            .fetch_one(pool)
            .await
            .expect("reclaimed comment job must expose its reclaim count");
    assert_eq!(reclaim_count, 1);
    assert_eq!(
        begin_post_comment_delivery(pool, &stale_job)
            .await
            .expect("stale delivery transition must execute"),
        CasResult::LeaseLost
    );
    assert_eq!(
        mark_post_comment_pre_send_failed(pool, &stale_job, CommentErrorKind::Transient)
            .await
            .expect("stale pre-send finalizer must execute"),
        CasResult::LeaseLost
    );
    assert_eq!(
        begin_post_comment_delivery(pool, &current_job)
            .await
            .expect("current delivery transition must execute"),
        CasResult::Applied
    );
    assert_eq!(
        finalize_test_comment(pool, &stale_job, 9001).await,
        CasResult::LeaseLost,
        "stale claim cannot finalize the newer delivery attempt"
    );
    let sending_state: (String, bool, bool) = query_as(
        "select status, sending_started_at is not null, lease_expires_at > now() from post_comment_jobs where id = $1",
    )
    .bind(current_job.id)
    .fetch_one(pool)
    .await
    .expect("sending job state must exist");
    assert_eq!(sending_state, ("sending".into(), true, true));

    query(
        "update post_comment_jobs set lease_expires_at = now() - interval '1 second' where id = $1",
    )
    .bind(current_job.id)
    .execute(pool)
    .await
    .expect("sending lease must expire");
    assert!(
        claim_next_post_comment_job(pool)
            .await
            .expect("claim after expired send must execute")
            .is_none(),
        "expired sending must become delivery_unknown rather than be reclaimed"
    );
    assert_job_state(
        pool,
        current_job.id,
        "delivery_unknown",
        Some("delivery_unknown"),
        false,
    )
    .await;

    let rejected_job = claim_created_job(pool, 3).await;
    assert_eq!(
        begin_post_comment_delivery(pool, &rejected_job)
            .await
            .unwrap(),
        CasResult::Applied
    );
    assert_eq!(
        mark_post_comment_send_rejected(
            pool,
            &rejected_job,
            CommentErrorKind::RateLimited,
            Some(120),
        )
        .await
        .expect("confirmed rejection must execute"),
        CasResult::Applied
    );
    let rejection_state: (String, bool) = query_as(
        "select status, next_attempt_at >= now() + interval '119 seconds' from post_comment_jobs where id = $1",
    )
    .bind(rejected_job.id)
    .fetch_one(pool)
    .await
    .expect("retry-after state must exist");
    assert_eq!(rejection_state, ("retry_wait".into(), true));

    let unknown_job = claim_created_job(pool, 4).await;
    assert_eq!(
        begin_post_comment_delivery(pool, &unknown_job)
            .await
            .unwrap(),
        CasResult::Applied
    );
    assert_eq!(
        mark_post_comment_delivery_unknown(pool, &unknown_job)
            .await
            .expect("ambiguous delivery finalizer must execute"),
        CasResult::Applied
    );
    assert_job_state(
        pool,
        unknown_job.id,
        "delivery_unknown",
        Some("delivery_unknown"),
        false,
    )
    .await;

    assert_comment_finalization_rolls_back_on_database_error(pool).await;
    assert_comment_finalization_is_idempotent_when_audit_rows_exist(pool).await;

    let sent_job = claim_created_job(pool, 5).await;
    assert_eq!(
        begin_post_comment_delivery(pool, &sent_job).await.unwrap(),
        CasResult::Applied
    );
    assert_eq!(
        finalize_test_comment(pool, &sent_job, 9002).await,
        CasResult::Applied
    );
    let final_counts: (String, Option<i32>, i64, i64) = query_as(
        "select j.status, j.bot_comment_message_id, (select count(*) from llm_generations where post_comment_job_id = j.id), (select count(*) from post_history_entries where post_comment_job_id = j.id) from post_comment_jobs j where j.id = $1",
    )
    .bind(sent_job.id)
    .fetch_one(pool)
    .await
    .expect("sent transaction effects must exist");
    assert_eq!(final_counts, ("sent".into(), Some(9002), 1, 1));
    assert_eq!(
        finalize_test_comment(pool, &sent_job, 9003).await,
        CasResult::LeaseLost,
        "a completed send cannot be finalized twice"
    );
}

async fn assert_comment_reconciliation_requires_operator_claim(pool: &PgPool) {
    let unknown_id = create_job(pool, 9_400_301).await;
    query("update post_comment_jobs set status = 'delivery_unknown', error_kind = 'delivery_unknown' where id = $1")
        .bind(unknown_id)
        .execute(pool).await.expect("ambiguous fixture must be created");
    assert!(
        claim_next_post_comment_job(pool)
            .await
            .expect("normal claim must execute")
            .is_none(),
        "normal worker must never automatically claim delivery_unknown"
    );

    let audit = OperatorAuditParams {
        actor: "postgres-test",
        reason: "confirmed delivery reconciliation",
    };
    assert_eq!(
        mark_delivery_unknown_post_comment_delivered(pool, unknown_id, 9_400_401, audit)
            .await
            .expect("operator delivered transition must execute"),
        CasResult::Applied
    );
    let delivered: (String, Option<i32>, i64) = query_as(
        "select status, bot_comment_message_id, (select count(*) from post_comment_job_operator_audit where post_comment_job_id = $1 and action = 'mark_delivered') from post_comment_jobs where id = $1",
    ).bind(unknown_id).fetch_one(pool).await.expect("delivered reconciliation must persist");
    assert_eq!(delivered, ("sent".to_string(), Some(9_400_401), 1));

    let retry_id = create_job(pool, 9_400_302).await;
    query("update post_comment_jobs set status = 'delivery_unknown', error_kind = 'delivery_unknown' where id = $1")
        .bind(retry_id).execute(pool).await.expect("retry fixture must be ambiguous");
    let retry = claim_delivery_unknown_post_comment_for_operator_retry(
        pool,
        retry_id,
        OperatorAuditParams {
            actor: "postgres-test",
            reason: "manual duplicate-risk retry",
        },
    )
    .await
    .expect("operator retry claim must execute")
    .expect("exact ambiguous job must be claimed");
    assert!(retry.operator_retry_only);
    assert!(
        claim_next_post_comment_job(pool)
            .await
            .expect("normal claim after operator claim must execute")
            .is_none(),
        "operator retry processing lease must remain outside automatic worker"
    );
    query(
        "update post_comment_jobs set lease_expires_at = now() - interval '1 second' where id = $1",
    )
    .bind(retry.id)
    .execute(pool)
    .await
    .expect("operator retry processing lease must be expirable");
    let recovered_retry = claim_next_post_comment_job(pool)
        .await
        .expect("expired operator retry must be safely recoverable before send")
        .expect("operator retry must be reclaimed after expiry");
    assert!(recovered_retry.operator_retry_only);
    assert!(recovered_retry.attempts > retry.attempts);
    assert_eq!(
        mark_operator_retry_post_comment_terminal_failed(
            pool,
            &recovered_retry,
            CommentErrorKind::Transient,
        )
        .await
        .expect("operator retry failure must execute"),
        CasResult::Applied
    );
    assert_job_state(pool, retry_id, "failed", Some("transient"), false).await;
    let retry_failure: (bool, i64) = query_as(
        "select operator_retry_only, (select count(*) from post_comment_job_operator_audit where post_comment_job_id = $1 and action = 'retry' and resulting_status = 'failed') from post_comment_jobs where id = $1",
    )
    .bind(retry_id)
    .fetch_one(pool)
    .await
    .expect("operator retry failure must clear its flag and persist an outcome audit");
    assert_eq!(retry_failure, (false, 1));

    let sent_id = create_job(pool, 9_400_304).await;
    query("update post_comment_jobs set status = 'delivery_unknown', error_kind = 'delivery_unknown' where id = $1")
        .bind(sent_id)
        .execute(pool)
        .await
        .expect("sent retry fixture must be ambiguous");
    let sent_retry = claim_delivery_unknown_post_comment_for_operator_retry(
        pool,
        sent_id,
        OperatorAuditParams {
            actor: "postgres-test",
            reason: "manual retry expected to send",
        },
    )
    .await
    .expect("operator sent retry claim must execute")
    .expect("ambiguous sent retry must be claimed");
    assert_eq!(
        begin_post_comment_delivery(pool, &sent_retry)
            .await
            .unwrap(),
        CasResult::Applied
    );
    assert_eq!(
        finalize_test_comment(pool, &sent_retry, 9_400_404).await,
        CasResult::Applied
    );
    let retry_success: (String, bool, i64) = query_as(
        "select status, operator_retry_only, (select count(*) from post_comment_job_operator_audit where post_comment_job_id = $1 and action = 'retry' and resulting_status = 'sent') from post_comment_jobs where id = $1",
    )
    .bind(sent_id)
    .fetch_one(pool)
    .await
    .expect("operator retry success must clear its flag and persist an outcome audit");
    assert_eq!(retry_success, ("sent".to_string(), false, 1));

    let unknown_retry_id = create_job(pool, 9_400_305).await;
    query("update post_comment_jobs set status = 'delivery_unknown', error_kind = 'delivery_unknown' where id = $1")
        .bind(unknown_retry_id)
        .execute(pool)
        .await
        .expect("unknown retry fixture must be ambiguous");
    let unknown_retry = claim_delivery_unknown_post_comment_for_operator_retry(
        pool,
        unknown_retry_id,
        OperatorAuditParams {
            actor: "postgres-test",
            reason: "manual retry may remain ambiguous",
        },
    )
    .await
    .expect("operator unknown retry claim must execute")
    .expect("ambiguous unknown retry must be claimed");
    assert_eq!(
        begin_post_comment_delivery(pool, &unknown_retry)
            .await
            .unwrap(),
        CasResult::Applied
    );
    assert_eq!(
        mark_post_comment_delivery_unknown(pool, &unknown_retry)
            .await
            .expect("operator retry ambiguity finalizer must execute"),
        CasResult::Applied
    );
    let retry_unknown: (String, bool, i64) = query_as(
        "select status, operator_retry_only, (select count(*) from post_comment_job_operator_audit where post_comment_job_id = $1 and action = 'retry' and resulting_status = 'delivery_unknown') from post_comment_jobs where id = $1",
    )
    .bind(unknown_retry_id)
    .fetch_one(pool)
    .await
    .expect("operator retry ambiguity must retain its flag and persist an outcome audit");
    assert_eq!(retry_unknown, ("delivery_unknown".to_string(), true, 1));

    let expired_send_id = create_job(pool, 9_400_306).await;
    query("update post_comment_jobs set status = 'delivery_unknown', error_kind = 'delivery_unknown' where id = $1")
        .bind(expired_send_id)
        .execute(pool)
        .await
        .expect("expired send retry fixture must be ambiguous");
    let expired_send_retry = claim_delivery_unknown_post_comment_for_operator_retry(
        pool,
        expired_send_id,
        OperatorAuditParams {
            actor: "postgres-test",
            reason: "manual retry whose send lease expires",
        },
    )
    .await
    .expect("operator expired-send retry claim must execute")
    .expect("ambiguous expired-send retry must be claimed");
    assert_eq!(
        begin_post_comment_delivery(pool, &expired_send_retry)
            .await
            .expect("operator retry must enter sending"),
        CasResult::Applied
    );
    query(
        "update post_comment_jobs set lease_expires_at = now() - interval '1 second' where id = $1",
    )
    .bind(expired_send_id)
    .execute(pool)
    .await
    .expect("operator retry sending lease must expire");
    assert!(
        claim_next_post_comment_job(pool)
            .await
            .expect("normal claim must expire sending operator retry")
            .is_none(),
        "expired sending operator retry must become delivery_unknown rather than be reclaimed"
    );
    let expired_send_outcome: (String, Option<String>, bool, i64) = query_as(
        "select status, error_kind, operator_retry_only, (select count(*) from post_comment_job_operator_audit where post_comment_job_id = $1 and action = 'retry' and previous_status = 'sending' and resulting_status = 'delivery_unknown') from post_comment_jobs where id = $1",
    )
    .bind(expired_send_id)
    .fetch_one(pool)
    .await
    .expect("expired sending operator retry must persist one outcome audit");
    assert_eq!(
        expired_send_outcome,
        (
            "delivery_unknown".to_string(),
            Some("delivery_unknown".to_string()),
            true,
            1
        )
    );

    let failed_id = create_job(pool, 9_400_303).await;
    query("update post_comment_jobs set status = 'delivery_unknown', error_kind = 'delivery_unknown' where id = $1")
        .bind(failed_id).execute(pool).await.expect("failed fixture must be ambiguous");
    assert_eq!(
        mark_delivery_unknown_post_comment_failed(
            pool,
            failed_id,
            OperatorAuditParams {
                actor: "postgres-test",
                reason: "confirmed not delivered"
            }
        )
        .await
        .expect("operator failed transition must execute"),
        CasResult::Applied
    );
    assert_job_state(
        pool,
        failed_id,
        "failed",
        Some("operator_marked_failed"),
        false,
    )
    .await;
}

async fn assert_comment_finalization_rolls_back_on_database_error(pool: &PgPool) {
    let job = claim_created_job(pool, 9_400_101).await;
    assert_eq!(
        begin_post_comment_delivery(pool, &job)
            .await
            .expect("delivery transition must execute"),
        CasResult::Applied
    );

    query("drop trigger if exists post_comment_history_failure_trigger on post_history_entries")
        .execute(pool)
        .await
        .expect("previous test trigger must be removable");
    query("drop function if exists post_comment_history_failure()")
        .execute(pool)
        .await
        .expect("previous test trigger function must be removable");
    query(
        r#"
        create function post_comment_history_failure()
        returns trigger
        language plpgsql
        as $$
        begin
            raise exception 'forced post history persistence failure';
        end;
        $$
        "#,
    )
    .execute(pool)
    .await
    .expect("failure trigger function must be created");
    query(
        r#"
        create trigger post_comment_history_failure_trigger
        before insert on post_history_entries
        for each row execute function post_comment_history_failure()
        "#,
    )
    .execute(pool)
    .await
    .expect("failure trigger must be created");

    let attempts = serde_json::json!([]);
    let result = finalize_post_comment_sent(
        pool,
        &job,
        FinalizePostCommentSent {
            bot_comment_message_id: 9_400_201,
            generation: LlmGenerationInsert {
                job_id: job.id,
                provider: "test",
                model: "test",
                prompt: "test prompt",
                image_used: false,
                response: "test comment",
                final_html: "test comment",
                attempts: &attempts,
                used_search_result_id: None,
                used_chat_message_ids: &[],
            },
            history_used_search_result: None,
            source_channel_id: job.source_channel_id,
            source_message_id: job.source_message_id,
            cleaned_post_text: &job.cleaned_post_text,
            bot_comment: "test comment",
        },
    )
    .await;

    query("drop trigger post_comment_history_failure_trigger on post_history_entries")
        .execute(pool)
        .await
        .expect("failure trigger must be cleaned up");
    query("drop function post_comment_history_failure()")
        .execute(pool)
        .await
        .expect("failure trigger function must be cleaned up");

    assert!(
        result.is_err(),
        "a true database persistence failure must fail finalization"
    );
    let status: String = query_scalar("select status from post_comment_jobs where id = $1")
        .bind(job.id)
        .fetch_one(pool)
        .await
        .expect("sending job must remain queryable after rollback");
    assert_eq!(status, "sending");
}

async fn assert_comment_finalization_is_idempotent_when_audit_rows_exist(pool: &PgPool) {
    let job = claim_created_job(pool, 9_400_102).await;
    assert_eq!(
        begin_post_comment_delivery(pool, &job)
            .await
            .expect("delivery transition must execute"),
        CasResult::Applied
    );
    query(
        r#"
        insert into llm_generations
            (post_comment_job_id, provider, model, prompt, image_used, response, final_html)
        values ($1, 'preexisting', 'test', 'test prompt', false, 'test comment', 'test comment')
        "#,
    )
    .bind(job.id)
    .execute(pool)
    .await
    .expect("matching preexisting generation must be inserted");
    query(
        r#"
        insert into post_history_entries
            (post_comment_job_id, source_channel_id, source_message_id, post_text, bot_comment)
        values ($1, $2, $3, 'Тестовый пост', 'test comment')
        "#,
    )
    .bind(job.id)
    .bind(job.source_channel_id)
    .bind(job.source_message_id)
    .execute(pool)
    .await
    .expect("matching preexisting history row must be inserted");

    assert_eq!(
        finalize_test_comment(pool, &job, 9_400_202).await,
        CasResult::Applied
    );
    let state: (String, i64, i64) = query_as(
        r#"
        select j.status,
               (select count(*) from llm_generations where post_comment_job_id = j.id),
               (select count(*) from post_history_entries where post_comment_job_id = j.id)
        from post_comment_jobs j
        where j.id = $1
        "#,
    )
    .bind(job.id)
    .fetch_one(pool)
    .await
    .expect("idempotently finalized job must be queryable");
    assert_eq!(state, ("sent".to_string(), 1, 1));
}

async fn claim_created_job(
    pool: &PgPool,
    sequence: i32,
) -> tg_ai_bot_teloxide::features::first_comment::repo::PostCommentJob {
    let id = create_job(pool, sequence).await;
    let job = claim_next_post_comment_job(pool)
        .await
        .expect("claim must succeed")
        .expect("created job must be claimable");
    assert_eq!(job.id, id);
    job
}

async fn finalize_test_comment(
    pool: &PgPool,
    job: &tg_ai_bot_teloxide::features::first_comment::repo::PostCommentJob,
    message_id: i32,
) -> CasResult {
    let attempts = serde_json::json!([]);
    finalize_post_comment_sent(
        pool,
        job,
        FinalizePostCommentSent {
            bot_comment_message_id: message_id,
            generation: LlmGenerationInsert {
                job_id: job.id,
                provider: "test",
                model: "test",
                prompt: "test prompt",
                image_used: false,
                response: "test comment",
                final_html: "test comment",
                attempts: &attempts,
                used_search_result_id: None,
                used_chat_message_ids: &[],
            },
            history_used_search_result: None,
            source_channel_id: job.source_channel_id,
            source_message_id: job.source_message_id,
            cleaned_post_text: &job.cleaned_post_text,
            bot_comment: "test comment",
        },
    )
    .await
    .expect("confirmed send finalization must execute")
}

async fn create_job(pool: &PgPool, sequence: i32) -> i64 {
    create_post_comment_job(
        pool,
        CreatePostCommentJobParams {
            discussion_chat_id: -1001932061163,
            discussion_message_id: sequence,
            source_channel_id: -1001575496091,
            source_message_id: sequence,
            cleaned_post_text: "Тестовый пост",
            image_file_id: None,
            image_file_unique_id: None,
        },
    )
    .await
    .expect("job insert must succeed")
    .expect("unique test job must be inserted")
}

async fn make_ready_for_retry(pool: &PgPool, job_id: i64) {
    query(
        "update post_comment_jobs set next_attempt_at = now() - interval '1 second' where id = $1",
    )
    .bind(job_id)
    .execute(pool)
    .await
    .expect("retry scheduling setup must succeed");
}

async fn assert_job_state(
    pool: &PgPool,
    job_id: i64,
    expected_status: &str,
    expected_error_kind: Option<&str>,
    expected_next_attempt_is_future: bool,
) {
    let (status, error_kind, next_attempt_is_future): (String, Option<String>, bool) = query_as(
        "select status, error_kind, next_attempt_at > now() from post_comment_jobs where id = $1",
    )
    .bind(job_id)
    .fetch_one(pool)
    .await
    .expect("job state must exist");

    assert_eq!(status, expected_status);
    assert_eq!(error_kind.as_deref(), expected_error_kind);
    assert_eq!(next_attempt_is_future, expected_next_attempt_is_future);
}
