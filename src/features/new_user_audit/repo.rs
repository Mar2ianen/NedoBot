use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::features::jobs::claim::CasResult;
use crate::features::jobs::policy::{ANALYSIS_RETRY, EXTERNAL_REQUEST_LEASE};
use crate::features::new_user_audit::scoring::ScoreComponents;

/// Версия правил записи unified score. Меняется только при несовместимом изменении
/// materializer-а, чтобы старые shadow assessments не применялись молча.
pub const CURRENT_MATERIALIZATION_VERSION: &str = "unified-audit-materialization-v1";

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
    pub avatar_file_id: Option<String>,
    pub avatar_file_unique_id: Option<String>,
    pub assessment_json: Option<Value>,
    pub attempts: i32,
    pub is_materialization_replay: bool,
}

pub struct NewUserAuditJobParams<'a> {
    pub chat_id: i64,
    pub telegram_user_id: i64,
    pub snapshot_hash: &'a str,
    pub prompt_version: &'a str,
    pub input_json: &'a Value,
    pub avatar_file_id: Option<&'a str>,
    pub avatar_file_unique_id: Option<&'a str>,
}

// В следующем slice enqueue вызывается из profile enrichment.
#[allow(dead_code)]
pub async fn enqueue_new_user_audit_job(
    pool: &PgPool,
    params: NewUserAuditJobParams<'_>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    enqueue_new_user_audit_job_in_transaction(&mut tx, params).await?;
    tx.commit().await?;
    Ok(())
}

/// Обновляет ревизию снимка и upsert-ит job в уже открытой транзакции.
///
/// Авторитетный baseline вызывает это вместе с сохранением аудита, поэтому
/// materializer не может увидеть новый baseline без соответствующего job.
pub async fn enqueue_new_user_audit_job_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    params: NewUserAuditJobParams<'_>,
) -> anyhow::Result<()> {
    sqlx::query(
        "update telegram_new_user_profile_audits set unified_audit_snapshot_hash = $3, unified_audit_generation = unified_audit_generation + 1 where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(params.chat_id)
    .bind(params.telegram_user_id)
    .bind(params.snapshot_hash)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        insert into new_user_audit_jobs
            (
                chat_id, telegram_user_id, snapshot_hash, prompt_version, input_json,
                avatar_file_id, avatar_file_unique_id, materialization_version
            )
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        on conflict (chat_id, telegram_user_id, snapshot_hash, prompt_version)
        do update set
            input_json = excluded.input_json,
            avatar_file_id = excluded.avatar_file_id,
            avatar_file_unique_id = excluded.avatar_file_unique_id,
            updated_at = now()
        "#,
    )
    .bind(params.chat_id)
    .bind(params.telegram_user_id)
    .bind(params.snapshot_hash)
    .bind(params.prompt_version)
    .bind(params.input_json)
    .bind(params.avatar_file_id)
    .bind(params.avatar_file_unique_id)
    .bind(CURRENT_MATERIALIZATION_VERSION)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

// В следующем slice claim вызывается из bounded unified audit worker.
#[allow(dead_code)]
pub async fn claim_next_new_user_audit_job(
    pool: &PgPool,
) -> anyhow::Result<Option<NewUserAuditJob>> {
    claim_next_new_user_audit_job_with_materialization(pool, false).await
}

/// В authoritative режиме успешные shadow jobs текущей версии могут быть
/// повторно leased исключительно для materialization, без генерации.
pub async fn claim_next_new_user_audit_job_with_materialization(
    pool: &PgPool,
    materialization_enabled: bool,
) -> anyhow::Result<Option<NewUserAuditJob>> {
    let row = sqlx::query(
        r#"
        with candidate as (
            select id, assessment_json is not null as is_materialization_replay
            from new_user_audit_jobs
            where (status in ('pending', 'retry_wait') and next_attempt_at <= now())
               or (status = 'processing' and lease_expires_at <= now())
               or (
                    $2
                    and status = 'succeeded'
                    and assessment_json is not null
                    and materialization_status = 'pending'
                    and materialization_version = $3
               )
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
                  job.prompt_version, job.input_json, job.avatar_file_id,
                  job.avatar_file_unique_id, job.assessment_json, job.attempts,
                  candidate.is_materialization_replay
        "#,
    )
    .bind(EXTERNAL_REQUEST_LEASE.seconds())
    .bind(materialization_enabled)
    .bind(CURRENT_MATERIALIZATION_VERSION)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| NewUserAuditJob {
        id: row.get("id"),
        chat_id: row.get("chat_id"),
        telegram_user_id: row.get("telegram_user_id"),
        snapshot_hash: row.get("snapshot_hash"),
        prompt_version: row.get("prompt_version"),
        input_json: row.get("input_json"),
        avatar_file_id: row.get("avatar_file_id"),
        avatar_file_unique_id: row.get("avatar_file_unique_id"),
        assessment_json: row.get("assessment_json"),
        attempts: row.get("attempts"),
        is_materialization_replay: row.get("is_materialization_replay"),
    }))
}

pub struct NewUserAuditOutcome<'a> {
    pub assessment_json: &'a Value,
    pub provider: &'a str,
    pub model: &'a str,
}

// В следующем slice finalizer вызывается unified audit service.
#[allow(dead_code)]
pub async fn finalize_new_user_audit_job(
    pool: &PgPool,
    job: &NewUserAuditJob,
    outcome: NewUserAuditOutcome<'_>,
) -> anyhow::Result<CasResult> {
    let update = sqlx::query(
        r#"
        update new_user_audit_jobs
        set status = 'succeeded', assessment_json = $3, provider = $4, model = $5,
            completed_at = now(), error_kind = null, lease_expires_at = null,
            updated_at = now()
        where id = $1 and attempts = $2 and status = 'processing'
        "#,
    )
    .bind(job.id)
    .bind(job.attempts)
    .bind(outcome.assessment_json)
    .bind(outcome.provider)
    .bind(outcome.model)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(update.rows_affected())
}

/// Завершает unified audit и материализует score/review в одной транзакции.
/// Telegram delivery намеренно не claim'ится здесь: её выполняет отдельный worker.
pub async fn finalize_authoritative_new_user_audit_job(
    pool: &PgPool,
    job: &NewUserAuditJob,
    outcome: NewUserAuditOutcome<'_>,
    components: &ScoreComponents,
) -> anyhow::Result<CasResult> {
    let mut tx = pool.begin().await?;
    let update = sqlx::query(
        r#"
        update new_user_audit_jobs
        set status = 'succeeded', assessment_json = $3, provider = $4, model = $5,
            completed_at = now(), error_kind = null, lease_expires_at = null, updated_at = now()
        where id = $1 and attempts = $2 and status = 'processing'
        "#,
    )
    .bind(job.id)
    .bind(job.attempts)
    .bind(outcome.assessment_json)
    .bind(outcome.provider)
    .bind(outcome.model)
    .execute(&mut *tx)
    .await?;
    let result = CasResult::from_rows_affected(update.rows_affected())?;
    if result == CasResult::LeaseLost {
        tx.rollback().await?;
        return Ok(result);
    }

    materialize_authoritative_in_transaction(&mut tx, job, components).await?;
    tx.commit().await?;
    Ok(result)
}

/// Применяет current-score к baseline только пока canonical snapshot не изменился.
async fn materialize_authoritative_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    job: &NewUserAuditJob,
    components: &ScoreComponents,
) -> anyhow::Result<()> {
    let final_score = components.final_score();
    let final_signals = components.final_signals();
    let audit_update = sqlx::query(
        r#"
        update telegram_new_user_profile_audits
        set risk_baseline_score = $3, risk_baseline_signals = $4,
            risk_first_message_score = $5, risk_first_message_signals = $6,
            risk_avatar_score = $7, risk_avatar_signals = $8,
            risk_score = $9, risk_level = $10, risk_signal_breakdown = $11
        where chat_id = $1 and telegram_user_id = $2
          and unified_audit_snapshot_hash = $12
        "#,
    )
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .bind(components.baseline_score)
    .bind(&components.baseline_signals)
    .bind(components.first_message_score)
    .bind(&components.first_message_signals)
    .bind(components.avatar_score)
    .bind(&components.avatar_signals)
    .bind(final_score)
    .bind(components.final_level())
    .bind(&final_signals)
    .bind(&job.snapshot_hash)
    .execute(&mut **tx)
    .await?;
    if audit_update.rows_affected() == 0 {
        sqlx::query("update new_user_audit_jobs set materialization_status = 'stale', materialized_at = now(), materialization_error_kind = 'snapshot_stale' where id = $1")
            .bind(job.id)
            .execute(&mut **tx)
            .await?;
        return Ok(());
    }

    sqlx::query(
        r#"
        insert into spam_review_requests (chat_id, telegram_user_id, risk_score, risk_signals)
        values ($1, $2, $3, $4)
        on conflict (chat_id, telegram_user_id) do update
        set risk_score = excluded.risk_score, risk_signals = excluded.risk_signals,
            notification_status = case when spam_review_requests.status = 'pending'
                and spam_review_requests.notification_status in ('pending', 'retry_wait', 'sent')
                and (spam_review_requests.notified_risk_score, spam_review_requests.notified_risk_signals)
                    is distinct from (excluded.risk_score, excluded.risk_signals)
                then 'retry_wait' else spam_review_requests.notification_status end,
            notification_next_attempt_at = case when spam_review_requests.status = 'pending'
                and spam_review_requests.notification_status in ('pending', 'retry_wait', 'sent')
                and (spam_review_requests.notified_risk_score, spam_review_requests.notified_risk_signals)
                    is distinct from (excluded.risk_score, excluded.risk_signals)
                then now() else spam_review_requests.notification_next_attempt_at end
        "#,
    )
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .bind(final_score)
    .bind(&final_signals)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "update new_user_audit_jobs set materialization_status = 'succeeded', materialized_at = now(), materialization_error_kind = null where id = $1",
    )
    .bind(job.id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Завершает lease успешной shadow job и materializes уже сохранённый assessment.
/// Assessment не перезаписывается, поэтому этот путь никогда не инициирует LLM generation.
pub async fn materialize_authoritative_new_user_audit_job(
    pool: &PgPool,
    job: &NewUserAuditJob,
    components: &ScoreComponents,
) -> anyhow::Result<CasResult> {
    let mut tx = pool.begin().await?;
    let update = sqlx::query(
        "update new_user_audit_jobs set status = 'succeeded', error_kind = null, lease_expires_at = null, updated_at = now() where id = $1 and attempts = $2 and status = 'processing' and assessment_json is not null and materialization_status = 'pending' and materialization_version = $3",
    )
    .bind(job.id)
    .bind(job.attempts)
    .bind(CURRENT_MATERIALIZATION_VERSION)
    .execute(&mut *tx)
    .await?;
    let result = CasResult::from_rows_affected(update.rows_affected())?;
    if result == CasResult::LeaseLost {
        tx.rollback().await?;
        return Ok(result);
    }
    materialize_authoritative_in_transaction(&mut tx, job, components).await?;
    tx.commit().await?;
    Ok(result)
}

/// Stops a malformed stored assessment from being retried as a generation job.
pub async fn mark_new_user_audit_materialization_stale(
    pool: &PgPool,
    job: &NewUserAuditJob,
    error_kind: &str,
) -> anyhow::Result<CasResult> {
    let update = sqlx::query(
        "update new_user_audit_jobs set status = 'succeeded', materialization_status = 'stale', materialized_at = now(), materialization_error_kind = $3, lease_expires_at = null, updated_at = now() where id = $1 and attempts = $2 and status = 'processing' and assessment_json is not null",
    )
    .bind(job.id)
    .bind(job.attempts)
    .bind(error_kind)
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
        set status = 'failed', error_kind = $3, completed_at = now(),
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
