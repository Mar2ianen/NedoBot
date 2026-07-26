use sqlx::{PgPool, query_scalar};

#[tokio::test]
#[ignore = "run with ./scripts/test.sh against the local test database"]
async fn clean_test_database_applies_all_migrations() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must be set by scripts/test.sh");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("local test database must be reachable");

    let migration_count: i64 = query_scalar("select count(*) from _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("migration ledger must exist");
    assert!(migration_count > 0, "all migrations must be applied");

    let post_comment_jobs: Option<String> =
        query_scalar("select to_regclass('public.post_comment_jobs')::text")
            .fetch_one(&pool)
            .await
            .expect("post_comment_jobs lookup must succeed");
    assert_eq!(post_comment_jobs.as_deref(), Some("post_comment_jobs"));

    let public_messages_view: Option<String> =
        query_scalar("select to_regclass('mcp_public.telegram_messages')::text")
            .fetch_one(&pool)
            .await
            .expect("public MCP view lookup must succeed");
    assert_eq!(
        public_messages_view.as_deref(),
        Some("mcp_public.telegram_messages")
    );
}
