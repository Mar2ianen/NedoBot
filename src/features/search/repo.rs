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
    let plan = serde_json::to_value(&context.plan)?;

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

    sqlx::query(
        r#"
        insert into chat_research_runs (post_comment_job_id, plan)
        values ($1, $2)
        on conflict (post_comment_job_id) do update set plan = excluded.plan, updated_at = now()
        "#,
    )
    .bind(post_comment_job_id)
    .bind(plan)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn save_chat_retrieval_candidates(
    pool: &PgPool,
    post_comment_job_id: i64,
    candidates: &impl serde::Serialize,
) -> anyhow::Result<()> {
    sqlx::query(
        "update chat_research_runs set retrieval_candidates = $2, updated_at = now() where post_comment_job_id = $1",
    )
    .bind(post_comment_job_id)
    .bind(serde_json::to_value(candidates)?)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn save_expanded_chat_contexts(
    pool: &PgPool,
    post_comment_job_id: i64,
    contexts: &impl serde::Serialize,
) -> anyhow::Result<()> {
    sqlx::query(
        "update chat_research_runs set expanded_contexts = $2, updated_at = now() where post_comment_job_id = $1",
    )
    .bind(post_comment_job_id)
    .bind(serde_json::to_value(contexts)?)
    .execute(pool)
    .await?;
    Ok(())
}
