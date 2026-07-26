use sqlx::{PgPool, query, query_as, query_scalar};
use tg_ai_bot_teloxide::features::{
    ask::notes::add_user_note_from_search,
    avatar_analysis::service::apply_avatar_risk_signal,
    chat_retrieval::enqueue_message_embedding_if_enabled,
    first_comment::repo::{
        CommentErrorKind, claim_next_post_comment_job, create_post_comment_job,
        mark_post_comment_failed, mark_post_comment_sent,
    },
    first_message_spam::enqueue_first_message_spam_analysis_if_enabled,
    spam_review::create_high_risk_review,
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
    assert_public_mcp_scope(&pool).await;
    assert_feature_gated_jobs(&pool).await;
    assert_agent_note_contract(&pool).await;
    assert_high_risk_review_deduplication(&pool).await;
    assert_comment_job_lifecycle(&pool).await;
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

async fn assert_high_risk_review_deduplication(pool: &PgPool) {
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

    let first_review = create_high_risk_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("review creation must succeed");
    assert!(
        first_review.is_some(),
        "high-risk audit must create a review"
    );
    let duplicate_review = create_high_risk_review(pool, CHAT_ID, USER_ID)
        .await
        .expect("duplicate review check must succeed");
    assert!(
        duplicate_review.is_none(),
        "review creation must be idempotent"
    );

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
    let retry_job_id = create_job(pool, 1).await;
    let retry_job = claim_next_post_comment_job(pool)
        .await
        .expect("claim must succeed")
        .expect("pending job must be claimed");
    assert_eq!(retry_job.id, retry_job_id);
    assert_eq!(retry_job.attempts, 1);
    assert!(
        mark_post_comment_failed(pool, &retry_job, CommentErrorKind::Transient)
            .await
            .expect("transient failure update must succeed")
    );
    assert_job_state(pool, retry_job.id, "retry_wait", Some("transient"), true).await;

    make_ready_for_retry(pool, retry_job.id).await;
    let retry_job = claim_next_post_comment_job(pool)
        .await
        .expect("second claim must succeed")
        .expect("retry job must be claimable");
    assert_eq!(retry_job.attempts, 2);
    assert!(
        mark_post_comment_failed(pool, &retry_job, CommentErrorKind::RateLimited)
            .await
            .expect("rate limit failure update must succeed")
    );
    assert_job_state(pool, retry_job.id, "retry_wait", Some("rate_limited"), true).await;

    make_ready_for_retry(pool, retry_job.id).await;
    let retry_job = claim_next_post_comment_job(pool)
        .await
        .expect("third claim must succeed")
        .expect("second retry job must be claimable");
    assert_eq!(retry_job.attempts, 3);
    assert!(
        mark_post_comment_failed(pool, &retry_job, CommentErrorKind::Transient)
            .await
            .expect("terminal transient failure update must succeed")
    );
    assert_job_state(pool, retry_job.id, "failed", Some("transient"), false).await;

    let configuration_job_id = create_job(pool, 2).await;
    let configuration_job = claim_next_post_comment_job(pool)
        .await
        .expect("configuration claim must succeed")
        .expect("configuration job must be claimable");
    assert_eq!(configuration_job.id, configuration_job_id);
    assert!(
        mark_post_comment_failed(pool, &configuration_job, CommentErrorKind::Configuration)
            .await
            .expect("configuration failure update must succeed")
    );
    assert_job_state(
        pool,
        configuration_job.id,
        "failed",
        Some("configuration"),
        false,
    )
    .await;

    let lease_job_id = create_job(pool, 3).await;
    let stale_job = claim_next_post_comment_job(pool)
        .await
        .expect("lease claim must succeed")
        .expect("lease job must be claimable");
    assert_eq!(stale_job.id, lease_job_id);
    query(
        "update post_comment_jobs set lease_expires_at = now() - interval '1 second' where id = $1",
    )
    .bind(lease_job_id)
    .execute(pool)
    .await
    .expect("lease expiry setup must succeed");

    let reclaimed_job = claim_next_post_comment_job(pool)
        .await
        .expect("expired lease claim must succeed")
        .expect("expired lease job must be reclaimed");
    assert_eq!(reclaimed_job.id, stale_job.id);
    assert_eq!(reclaimed_job.attempts, stale_job.attempts + 1);
    assert!(
        !mark_post_comment_sent(pool, &stale_job, 9001)
            .await
            .expect("stale finalization query must succeed")
    );
    assert!(
        !mark_post_comment_failed(pool, &stale_job, CommentErrorKind::Transient)
            .await
            .expect("stale failure query must succeed")
    );
    assert!(
        mark_post_comment_sent(pool, &reclaimed_job, 9002)
            .await
            .expect("current worker finalization query must succeed")
    );
    assert_job_state(pool, reclaimed_job.id, "sent", None, false).await;
}

async fn create_job(pool: &PgPool, sequence: i32) -> i64 {
    create_post_comment_job(
        pool,
        -1001932061163,
        sequence,
        -1001575496091,
        sequence,
        "Тестовый пост",
        None,
        None,
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
