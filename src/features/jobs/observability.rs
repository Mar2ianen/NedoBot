use std::collections::BTreeMap;

use sqlx::{PgPool, Postgres, Transaction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStatusMetrics {
    pub status: String,
    pub jobs: i64,
    pub attempts: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobErrorMetrics {
    pub error_kind: String,
    pub jobs: i64,
    pub attempts: i64,
    pub terminal_failures: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobQueueMetrics {
    pub queue: &'static str,
    pub statuses: Vec<JobStatusMetrics>,
    /// Seconds since the earliest due initial/retry delivery, excluding expired leases.
    pub oldest_ready_age_seconds: Option<f64>,
    pub lease_reclaim_count: i64,
    pub errors: Vec<JobErrorMetrics>,
    /// Present only for the embeddings queue. This is a subset of `errors`.
    pub embedding_batch_cardinality_failures: Option<i64>,
}

const POST_COMMENT_STATUS_SQL: &str = r#"
    select status, count(*)::bigint, coalesce(sum(attempts), 0)::bigint
    from post_comment_jobs
    group by status
    order by status
"#;
const POST_COMMENT_ERROR_SQL: &str = r#"
    select error_kind, count(*)::bigint, coalesce(sum(attempts), 0)::bigint,
           count(*) filter (where status = 'failed')::bigint
    from post_comment_jobs
    where error_kind is not null
    group by error_kind
    order by error_kind
"#;
const POST_COMMENT_READY_AGE_SQL: &str = r#"
    select extract(epoch from now() - min(next_attempt_at))::double precision
    from post_comment_jobs
    where status in ('pending', 'retry_wait') and next_attempt_at <= now()
"#;
const POST_COMMENT_RECLAIMS_SQL: &str =
    "select coalesce(sum(lease_reclaim_count), 0)::bigint from post_comment_jobs";

const EMBEDDING_STATUS_SQL: &str = r#"
    select status, count(*)::bigint, coalesce(sum(attempts), 0)::bigint
    from telegram_message_embeddings_gemma
    group by status
    order by status
"#;
const EMBEDDING_ERROR_SQL: &str = r#"
    select error_kind, count(*)::bigint, coalesce(sum(attempts), 0)::bigint,
           count(*) filter (where status = 'failed')::bigint
    from telegram_message_embeddings_gemma
    where error_kind is not null
    group by error_kind
    order by error_kind
"#;
const EMBEDDING_READY_AGE_SQL: &str = r#"
    select extract(epoch from now() - min(next_attempt_at))::double precision
    from telegram_message_embeddings_gemma
    where status in ('pending', 'retry_wait') and next_attempt_at <= now()
"#;
const EMBEDDING_RECLAIMS_SQL: &str =
    "select coalesce(sum(lease_reclaim_count), 0)::bigint from telegram_message_embeddings_gemma";

const HISTORY_STATUS_SQL: &str = r#"
    select status, count(*)::bigint, coalesce(sum(attempts), 0)::bigint
    from post_history_entries
    group by status
    order by status
"#;
const HISTORY_ERROR_SQL: &str = r#"
    select error_kind, count(*)::bigint, coalesce(sum(attempts), 0)::bigint,
           count(*) filter (where status = 'failed')::bigint
    from post_history_entries
    where error_kind is not null
    group by error_kind
    order by error_kind
"#;
const HISTORY_READY_AGE_SQL: &str = r#"
    select extract(epoch from now() - min(next_attempt_at))::double precision
    from post_history_entries
    where status in ('pending', 'retry') and next_attempt_at <= now()
"#;
const HISTORY_RECLAIMS_SQL: &str =
    "select coalesce(sum(lease_reclaim_count), 0)::bigint from post_history_entries";

const REVIEW_STATUS_SQL: &str = r#"
    select notification_status, count(*)::bigint,
           coalesce(sum(notification_attempts), 0)::bigint
    from spam_review_requests
    group by notification_status
    order by notification_status
"#;
const REVIEW_ERROR_SQL: &str = r#"
    select notification_error_kind, count(*)::bigint,
           coalesce(sum(notification_attempts), 0)::bigint,
           count(*) filter (where notification_status = 'failed')::bigint
    from spam_review_requests
    where notification_error_kind is not null
    group by notification_error_kind
    order by notification_error_kind
"#;
const REVIEW_READY_AGE_SQL: &str = r#"
    select extract(epoch from now() - min(notification_next_attempt_at))::double precision
    from spam_review_requests
    where status = 'pending'
      and risk_score >= 70
      and notification_status in ('pending', 'retry_wait')
      and notification_next_attempt_at <= now()
"#;
const REVIEW_RECLAIMS_SQL: &str =
    "select coalesce(sum(notification_lease_reclaim_count), 0)::bigint from spam_review_requests";

struct QueueDefinition {
    name: &'static str,
    status_sql: &'static str,
    error_sql: &'static str,
    ready_age_sql: &'static str,
    reclaim_sql: &'static str,
    is_embedding: bool,
}

const QUEUES: [QueueDefinition; 4] = [
    QueueDefinition {
        name: "first-comments",
        status_sql: POST_COMMENT_STATUS_SQL,
        error_sql: POST_COMMENT_ERROR_SQL,
        ready_age_sql: POST_COMMENT_READY_AGE_SQL,
        reclaim_sql: POST_COMMENT_RECLAIMS_SQL,
        is_embedding: false,
    },
    QueueDefinition {
        name: "embeddings",
        status_sql: EMBEDDING_STATUS_SQL,
        error_sql: EMBEDDING_ERROR_SQL,
        ready_age_sql: EMBEDDING_READY_AGE_SQL,
        reclaim_sql: EMBEDDING_RECLAIMS_SQL,
        is_embedding: true,
    },
    QueueDefinition {
        name: "post-history",
        status_sql: HISTORY_STATUS_SQL,
        error_sql: HISTORY_ERROR_SQL,
        ready_age_sql: HISTORY_READY_AGE_SQL,
        reclaim_sql: HISTORY_RECLAIMS_SQL,
        is_embedding: false,
    },
    QueueDefinition {
        name: "reviews",
        status_sql: REVIEW_STATUS_SQL,
        error_sql: REVIEW_ERROR_SQL,
        ready_age_sql: REVIEW_READY_AGE_SQL,
        reclaim_sql: REVIEW_RECLAIMS_SQL,
        is_embedding: false,
    },
];

/// Loads the operational lifecycle projection in one read-only transaction.
/// SQL is fixed here rather than accepted from CLI input.
pub async fn load_job_lifecycle_report(pool: &PgPool) -> anyhow::Result<Vec<JobQueueMetrics>> {
    let mut transaction = pool.begin().await?;
    sqlx::query("set transaction read only")
        .execute(&mut *transaction)
        .await?;

    let mut report = Vec::with_capacity(QUEUES.len());
    for definition in &QUEUES {
        report.push(load_queue_metrics(&mut transaction, definition).await?);
    }
    transaction.commit().await?;
    Ok(report)
}

async fn load_queue_metrics(
    transaction: &mut Transaction<'_, Postgres>,
    definition: &QueueDefinition,
) -> anyhow::Result<JobQueueMetrics> {
    let statuses = sqlx::query_as::<_, (String, i64, i64)>(definition.status_sql)
        .fetch_all(&mut **transaction)
        .await?
        .into_iter()
        .map(|(status, jobs, attempts)| JobStatusMetrics {
            status,
            jobs,
            attempts,
        })
        .collect();
    let errors = normalize_error_metrics(
        sqlx::query_as::<_, (String, i64, i64, i64)>(definition.error_sql)
            .fetch_all(&mut **transaction)
            .await?,
    );
    let oldest_ready_age_seconds = sqlx::query_scalar::<_, Option<f64>>(definition.ready_age_sql)
        .fetch_one(&mut **transaction)
        .await?;
    let lease_reclaim_count = sqlx::query_scalar::<_, i64>(definition.reclaim_sql)
        .fetch_one(&mut **transaction)
        .await?;
    let embedding_batch_cardinality_failures = definition.is_embedding.then(|| {
        errors
            .iter()
            .find(|metric| metric.error_kind == "embedding_batch_cardinality")
            .map_or(0, |metric| metric.jobs)
    });

    Ok(JobQueueMetrics {
        queue: definition.name,
        statuses,
        oldest_ready_age_seconds,
        lease_reclaim_count,
        errors,
        embedding_batch_cardinality_failures,
    })
}

fn normalize_error_metrics(rows: Vec<(String, i64, i64, i64)>) -> Vec<JobErrorMetrics> {
    let mut grouped = BTreeMap::<String, JobErrorMetrics>::new();
    for (error_kind, jobs, attempts, terminal_failures) in rows {
        let error_kind = safe_error_kind(&error_kind).to_string();
        let metric = grouped
            .entry(error_kind.clone())
            .or_insert(JobErrorMetrics {
                error_kind,
                jobs: 0,
                attempts: 0,
                terminal_failures: 0,
            });
        metric.jobs += jobs;
        metric.attempts += attempts;
        metric.terminal_failures += terminal_failures;
    }
    grouped.into_values().collect()
}

const SAFE_ERROR_KINDS: &[&str] = &[
    "configuration",
    "invalid_input",
    "image_unavailable",
    "rate_limited",
    "transient",
    "delivery_unknown",
    "http_429",
    "embedding_failed",
    "embedding_batch_cardinality",
    "timeout",
    "validation_failed",
    "error",
    "telegram_invalid_token",
    "telegram_forbidden",
    "telegram_send_failed",
    "telegram_retry_exhausted",
    "telegram_message_missing",
    "operator_marked_failed",
];

fn safe_error_kind(error_kind: &str) -> &'static str {
    SAFE_ERROR_KINDS
        .iter()
        .copied()
        .find(|known| *known == error_kind)
        .unwrap_or("other")
}

#[cfg(test)]
mod tests {
    use super::{normalize_error_metrics, safe_error_kind};

    #[test]
    fn redacts_unknown_persisted_error_kinds() {
        assert_eq!(safe_error_kind("provider body: secret"), "other");
        let metrics = normalize_error_metrics(vec![
            ("provider body: secret".to_string(), 1, 2, 1),
            ("unclassified".to_string(), 2, 3, 0),
        ]);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].error_kind, "other");
        assert_eq!(metrics[0].jobs, 3);
        assert_eq!(metrics[0].attempts, 5);
        assert_eq!(metrics[0].terminal_failures, 1);
    }
}
