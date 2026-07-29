use chrono::{Duration, TimeZone, Utc};
use sqlx::{PgPool, query, query_as, query_scalar};
use tg_ai_bot_teloxide::features::{
    ask::notes::add_user_note_from_search,
    avatar_analysis::{
        repo::{
            claim_next_avatar_analysis_job, enqueue_avatar_analysis_job,
            mark_avatar_analysis_failed,
        },
        service::apply_avatar_risk_signal,
    },
    chat_retrieval::{
        EmbeddingJob, enqueue_message_embedding_if_enabled, mark_embedding_failed,
        mark_embedding_ready,
    },
    first_comment::repo::{
        CommentErrorKind, CreatePostCommentJobParams, FinalizePostCommentSent, LlmGenerationInsert,
        begin_post_comment_delivery, claim_next_post_comment_job, create_post_comment_job,
        finalize_post_comment_sent, mark_post_comment_delivery_unknown,
        mark_post_comment_pre_send_failed, mark_post_comment_send_rejected,
    },
    first_message_spam::enqueue_first_message_spam_analysis_if_enabled,
    jobs::claim::CasResult,
    spam_review::{claim_next_review_delivery, create_review, mark_review_delivery_succeeded},
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
    assert_spam_review_safety_backfill_upgrade(&pool).await;
    assert_sent_comment_requires_sent_at(&pool).await;
    assert_public_mcp_scope(&pool).await;
    assert_stats_renderers_share_period_data(&pool).await;
    assert_feature_gated_jobs(&pool).await;
    assert_agent_note_contract(&pool).await;
    assert_review_deduplication(&pool).await;
    assert_review_delivery_finalization_requires_current_claim(&pool).await;
    assert_terminal_review_delivery_stays_closed(&pool).await;
    assert_comment_job_lifecycle(&pool).await;
    assert_avatar_job_finalization_requires_current_claim(&pool).await;
    assert_embedding_job_finalization_requires_current_claim(&pool).await;
}

async fn assert_embedding_job_finalization_requires_current_claim(pool: &PgPool) {
    const CHAT_ID: i64 = -1001932061163;
    const STALE_MESSAGE_ID: i32 = 9_300_001;
    const RETRY_MESSAGE_ID: i32 = 9_300_002;
    const FAILED_MESSAGE_ID: i32 = 9_300_003;
    const TEXT: &str = "embedding finalization CAS";

    for message_id in [STALE_MESSAGE_ID, RETRY_MESSAGE_ID, FAILED_MESSAGE_ID] {
        query("insert into telegram_messages (chat_id, message_id, text) values ($1, $2, $3)")
            .bind(CHAT_ID)
            .bind(message_id)
            .bind(TEXT)
            .execute(pool)
            .await
            .expect("embedding source message must be inserted");
    }
    query(
        "insert into telegram_message_embeddings (chat_id, message_id, status, attempts, processing_started_at, lease_expires_at) values ($1, $2, 'processing', 2, now(), now() + interval '10 minutes')",
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

    let current_claim = EmbeddingJob {
        attempts: 2,
        ..stale_claim
    };
    assert_eq!(
        mark_embedding_ready(pool, &current_claim, &vec![0.0; 312], "test-model")
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

async fn assert_avatar_job_finalization_requires_current_claim(pool: &PgPool) {
    let user_id = 9_000_001_i64;
    let unique_id = "avatar-cas-regression";
    let features = serde_json::json!({ "test": true });
    query("insert into telegram_user_profiles (telegram_user_id, first_name) values ($1, 'Avatar CAS')")
        .bind(user_id)
        .execute(pool)
        .await
        .expect("avatar job user profile must exist");
    enqueue_avatar_analysis_job(
        pool,
        user_id,
        "avatar-file-id",
        unique_id,
        "avatar-snapshot",
        &features,
        "test-prompt",
    )
    .await
    .expect("avatar job must be enqueued");

    let first_claim = claim_next_avatar_analysis_job(pool)
        .await
        .expect("first avatar claim must succeed")
        .expect("avatar job must be claimed");
    query("update avatar_analysis_jobs set lease_expires_at = now() - interval '1 second' where id = $1")
        .bind(first_claim.id)
        .execute(pool)
        .await
        .expect("avatar lease must be expired for regression test");
    let second_claim = claim_next_avatar_analysis_job(pool)
        .await
        .expect("reclaimed avatar job must be claimable")
        .expect("avatar job must be reclaimed");
    assert!(second_claim.attempts > first_claim.attempts);

    let stale_result = mark_avatar_analysis_failed(pool, &first_claim, "test_failure", None)
        .await
        .expect("stale avatar finalization query must execute");
    assert_eq!(
        stale_result,
        tg_ai_bot_teloxide::features::jobs::claim::CasResult::LeaseLost
    );

    let status: (String, i32) =
        query_as("select status, attempts from avatar_analysis_jobs where id = $1")
            .bind(second_claim.id)
            .fetch_one(pool)
            .await
            .expect("reclaimed avatar job must remain present");
    assert_eq!(status, ("processing".to_string(), second_claim.attempts));

    let current_result = mark_avatar_analysis_failed(pool, &second_claim, "test_failure", None)
        .await
        .expect("current avatar finalization query must execute");
    assert_eq!(
        current_result,
        tg_ai_bot_teloxide::features::jobs::claim::CasResult::Applied
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

    let html = render_html::chat_stats(&data);
    let rich = render_rich::chat_stats(&data, CHAT_ID);
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

    enqueue_first_message_spam_analysis_if_enabled(pool, false, CHAT_ID, USER_ID)
        .await
        .expect("disabled first-message spam gate must succeed");
    let spam_jobs: i64 = query_scalar(
        "select count(*) from first_message_spam_analysis_jobs where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("spam job count query must succeed");
    assert_eq!(spam_jobs, 0);

    enqueue_first_message_spam_analysis_if_enabled(pool, true, CHAT_ID, USER_ID)
        .await
        .expect("enabled first-message spam gate must succeed");
    let spam_jobs: i64 = query_scalar(
        "select count(*) from first_message_spam_analysis_jobs where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("enabled spam job count query must succeed");
    assert_eq!(spam_jobs, 1);
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
            (chat_id, telegram_user_id, risk_score, risk_level, risk_signal_breakdown)
        values ($1, $2, 65, 'medium', '[]'::jsonb)
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
        "update spam_review_requests set notification_status = 'sent', notification_message_id = 900, notified_risk_score = 65, notified_risk_signals = '[]'::jsonb, notification_lease_expires_at = null where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .execute(pool)
    .await
    .expect("initial review delivery must be recorded");

    let duplicate_review = create_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("duplicate review check must succeed");
    assert!(
        duplicate_review.is_none(),
        "review creation must be idempotent"
    );

    let affected_chat_ids = apply_avatar_risk_signal(
        pool,
        USER_ID,
        "photo-42",
        &serde_json::json!({
            "primary_class": "suggestive_bait",
            "personal_photo_probability": 0.0,
        }),
    )
    .await
    .expect("avatar risk signal must be applied");
    assert_eq!(affected_chat_ids, vec![CHAT_ID]);
    let updated_review = create_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("avatar risk must refresh review delivery")
        .expect("changed sent review must be claimed for an edit");
    assert_eq!(updated_review.notification_message_id, Some(900));

    let (risk_score, risk_level): (i32, String) = query_as(
        "select risk_score, risk_level from telegram_new_user_profile_audits where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("updated risk audit must exist");
    assert_eq!(risk_score, 73);
    assert_eq!(risk_level, "high");

    let (review_score, review_signals): (i32, serde_json::Value) = query_as(
        "select risk_score, risk_signals from spam_review_requests where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("review snapshot must be refreshed after an avatar risk signal");
    assert_eq!(review_score, 73);
    assert!(
        review_signals
            .as_array()
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("label").and_then(serde_json::Value::as_str)
                    == Some("suggestive_avatar_bait")
            })),
        "review snapshot must include the later avatar signal: {review_signals}"
    );

    let (notification_status, notification_attempts, notification_message_id): (String, i32, Option<i32>) = query_as(
        "select notification_status, notification_attempts, notification_message_id from spam_review_requests where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(CHAT_ID)
    .bind(USER_ID)
    .fetch_one(pool)
    .await
    .expect("changed sent review must be queued for an edit");
    assert_eq!(notification_status, "processing");
    assert_eq!(notification_attempts, 1);
    assert_eq!(notification_message_id, Some(900));

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
    assert_eq!(notification_message_id, Some(900));

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
