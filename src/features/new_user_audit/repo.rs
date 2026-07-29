use serde_json::Value;
use sqlx::{PgPool, Row};

use crate::features::jobs::claim::CasResult;
use crate::features::jobs::policy::{ANALYSIS_RETRY, EXTERNAL_REQUEST_LEASE};

// В следующем slice job будет подключена к worker; пока API вызывается PostgreSQL integration test.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct NewUserAuditJob {
    pub id: i64,
    pub chat_id: i64,
    pub telegram_user_id: i64,
    pub snapshot_hash: String,
    pub prompt_version: String,
    pub input_json: Value,
    pub attempts: i32,
}

// В следующем slice enqueue вызывается из profile enrichment.
#[allow(dead_code)]
pub async fn enqueue_new_user_audit_job(
    pool: &PgPool,
    chat_id: i64,
    telegram_user_id: i64,
    snapshot_hash: &str,
    prompt_version: &str,
    input_json: &Value,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into new_user_audit_jobs
            (chat_id, telegram_user_id, snapshot_hash, prompt_version, input_json)
        values ($1, $2, $3, $4, $5)
        on conflict (chat_id, telegram_user_id, snapshot_hash, prompt_version)
        do update set
            input_json = excluded.input_json,
            updated_at = now()
        "#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .bind(snapshot_hash)
    .bind(prompt_version)
    .bind(input_json)
    .execute(pool)
    .await?;
    Ok(())
}

// В следующем slice claim вызывается из bounded unified audit worker.
#[allow(dead_code)]
pub async fn claim_next_new_user_audit_job(
    pool: &PgPool,
) -> anyhow::Result<Option<NewUserAuditJob>> {
    let row = sqlx::query(
        r#"
        with candidate as (
            select id
            from new_user_audit_jobs
            where (status in ('pending', 'retry_wait') and next_attempt_at <= now())
               or (status = 'processing' and lease_expires_at <= now())
            order by next_attempt_at, id
            for update skip locked
            limit 1
        )
        update new_user_audit_jobs job
        set status = 'processing',
            attempts = job.attempts + 1,
            processing_started_at = now(),
            lease_expires_at = now() + ($1 * interval '1 second'),
            updated_at = now()
        from candidate
        where job.id = candidate.id
        returning job.id, job.chat_id, job.telegram_user_id, job.snapshot_hash,
                  job.prompt_version, job.input_json, job.attempts
        "#,
    )
    .bind(EXTERNAL_REQUEST_LEASE.seconds())
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| NewUserAuditJob {
        id: row.get("id"),
        chat_id: row.get("chat_id"),
        telegram_user_id: row.get("telegram_user_id"),
        snapshot_hash: row.get("snapshot_hash"),
        prompt_version: row.get("prompt_version"),
        input_json: row.get("input_json"),
        attempts: row.get("attempts"),
    }))
}

// В следующем slice success finalizer вызывается unified audit service.
#[allow(dead_code)]
pub async fn mark_new_user_audit_succeeded(
    pool: &PgPool,
    job: &NewUserAuditJob,
) -> anyhow::Result<CasResult> {
    let update = sqlx::query(
        r#"
        update new_user_audit_jobs
        set status = 'succeeded', error_kind = null,
            lease_expires_at = null, updated_at = now()
        where id = $1 and attempts = $2 and status = 'processing'
        "#,
    )
    .bind(job.id)
    .bind(job.attempts)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(update.rows_affected())
}

// В следующем slice retry finalizer вызывается unified audit service.
#[allow(dead_code)]
pub async fn mark_new_user_audit_retry(
    pool: &PgPool,
    job: &NewUserAuditJob,
    error_kind: &str,
    retry_after_seconds: Option<i64>,
) -> anyhow::Result<CasResult> {
    let Some(delay_seconds) = ANALYSIS_RETRY.delay_seconds(job.attempts, retry_after_seconds)
    else {
        return mark_new_user_audit_failed(pool, job, error_kind).await;
    };

    let update = sqlx::query(
        r#"
        update new_user_audit_jobs
        set status = 'retry_wait', error_kind = $3,
            next_attempt_at = now() + ($4 * interval '1 second'),
            lease_expires_at = null, updated_at = now()
        where id = $1 and attempts = $2 and status = 'processing'
        "#,
    )
    .bind(job.id)
    .bind(job.attempts)
    .bind(error_kind)
    .bind(delay_seconds)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(update.rows_affected())
}

// В следующем slice terminal finalizer вызывается unified audit service.
#[allow(dead_code)]
pub async fn mark_new_user_audit_failed(
    pool: &PgPool,
    job: &NewUserAuditJob,
    error_kind: &str,
) -> anyhow::Result<CasResult> {
    let update = sqlx::query(
        r#"
        update new_user_audit_jobs
        set status = 'failed', error_kind = $3,
            lease_expires_at = null, updated_at = now()
        where id = $1 and attempts = $2 and status = 'processing'
        "#,
    )
    .bind(job.id)
    .bind(job.attempts)
    .bind(error_kind)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(update.rows_affected())
}
