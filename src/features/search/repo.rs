use sqlx::PgPool;

use crate::features::search::types::SearchContext;

pub async fn insert_search_run(
    pool: &PgPool,
    post_comment_job_id: i64,
    context: &SearchContext,
) -> anyhow::Result<()> {
    let status = if context.is_skipped() {
        "skipped"
    } else {
        "completed"
    };
    let latency_ms = i64::try_from(context.latency_ms).unwrap_or(i64::MAX);
    let queries = serde_json::to_value(&context.queries)?;
    let results = serde_json::to_value(&context.results)?;

    sqlx::query(
        r#"
        insert into search_runs
            (post_comment_job_id, status, skipped_reason, latency_ms, queries, results)
        values ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(post_comment_job_id)
    .bind(status)
    .bind(context.skipped_reason.as_deref())
    .bind(latency_ms)
    .bind(queries)
    .bind(results)
    .execute(pool)
    .await?;

    Ok(())
}
