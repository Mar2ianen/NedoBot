use std::time::Duration;

use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::features::jobs::claim::CasResult;
use crate::features::jobs::policy::{
    ANALYSIS_RETRY, EXTERNAL_REQUEST_LEASE, MATERIALIZATION_RETRY,
};
use teloxide_antispam::scoring::ScoreComponents;

/// Версия правил записи unified score. Меняется при изменении scoring/materializer,
/// чтобы выбранные сохранённые assessments можно было безопасно переиграть.
pub const CURRENT_MATERIALIZATION_VERSION: &str = "unified-audit-materialization-v9";

/// Короткий retry для успешного LLM-ответа, ещё не пересёкшего durable
/// generation boundary. Job остаётся под исходным generation lease.
const GENERATION_FINALIZATION_SQL_RETRIES: u32 = 3;
const GENERATION_FINALIZATION_RETRY_DELAY: Duration = Duration::from_millis(100);

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
    pub review_threshold: i32,
    /// Monotonic generation claim sequence, used only by generation finalizers.
    pub attempts: i32,
    /// Monotonic lease fences; unlike retry counters these are never reset.
    pub generation_fence: i64,
    pub materialization_attempts: i32,
    pub materialization_fence: i64,
    pub audit_generation: i64,
    pub is_materialization_replay: bool,
}

#[derive(Clone, Copy)]
pub struct NewUserAuditJobParams<'a> {
    pub chat_id: i64,
    pub telegram_user_id: i64,
    pub snapshot_hash: &'a str,
    pub prompt_version: &'a str,
    pub input_json: &'a Value,
    pub avatar_file_id: Option<&'a str>,
    pub avatar_file_unique_id: Option<&'a str>,
    pub review_threshold: i32,
}

/// Returns whether an assessment covering this first text has been applied to
/// the baseline. A profile-only or still-pending materialization is not enough.
pub async fn has_materialized_audit_for_first_message(
    pool: &PgPool,
    chat_id: i64,
    telegram_user_id: i64,
    first_message_preview: Option<&str>,
) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar(
        r#"
        select exists(
            select 1
            from new_user_audit_jobs
            where chat_id = $1 and telegram_user_id = $2
              and status = 'succeeded'
              and materialization_status = 'succeeded'
              and input_json #>> '{text,first_message_preview}' is not distinct from $3
              and (
                    ($3::text is null and assessment_json->'first_message_assessment' = 'null'::jsonb)
                    or ($3::text is not null
                        and assessment_json->'first_message_assessment' is not null
                        and assessment_json->'first_message_assessment' <> 'null'::jsonb)
              )
        )
        "#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .bind(first_message_preview)
    .fetch_one(pool)
    .await?)
}

pub async fn latest_audit_trigger_fingerprint(
    pool: &PgPool,
    chat_id: i64,
    telegram_user_id: i64,
) -> anyhow::Result<Option<(String, String, bool)>> {
    Ok(sqlx::query_as(
        r#"select input_json->>'audit_trigger_fingerprint', status,
                  assessment_json is not null
           from new_user_audit_jobs
           where chat_id = $1 and telegram_user_id = $2
             and input_json->>'audit_trigger_fingerprint' is not null
           order by updated_at desc, id desc limit 1"#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn latest_audit_link_message_id(
    pool: &PgPool,
    chat_id: i64,
    telegram_user_id: i64,
) -> anyhow::Result<Option<i32>> {
    Ok(sqlx::query_scalar(
        r#"select nullif(input_json->>'latest_link_message_id', '')::integer
           from new_user_audit_jobs
           where chat_id = $1 and telegram_user_id = $2
             and input_json->>'audit_trigger_fingerprint' is not null
           order by updated_at desc, id desc limit 1"#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .fetch_optional(pool)
    .await?
    .flatten())
}

/// Moves a still-running generation to a newer material snapshot when the
/// triggering evidence and frozen deterministic rule are unchanged. The LLM
/// request remains unique while its eventual materialization targets latest data.
pub async fn refresh_in_flight_audit_snapshot_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    params: NewUserAuditJobParams<'_>,
) -> anyhow::Result<bool> {
    let updated = sqlx::query(
        r#"with candidate as (
               select id from new_user_audit_jobs
               where chat_id = $1 and telegram_user_id = $2
                 and snapshot_hash <> $3 and prompt_version = $4
                 and status in ('pending', 'retry_wait', 'processing')
                 and assessment_json is null
                 and input_json->>'audit_trigger_fingerprint' = $5
                 and input_json->>'deterministic_campaign_rule'
                       is not distinct from $6::text
                 and not exists (
                     select 1 from new_user_audit_jobs existing
                     where existing.chat_id = $1 and existing.telegram_user_id = $2
                       and existing.snapshot_hash = $3 and existing.prompt_version = $4
                 )
               order by updated_at desc, id desc
               for update skip locked limit 1
           )
           update new_user_audit_jobs job
           set snapshot_hash = $3, input_json = $7,
               avatar_file_id = $8, avatar_file_unique_id = $9,
               review_threshold = $10, updated_at = now()
           from candidate where job.id = candidate.id"#,
    )
    .bind(params.chat_id)
    .bind(params.telegram_user_id)
    .bind(params.snapshot_hash)
    .bind(params.prompt_version)
    .bind(
        params
            .input_json
            .get("audit_trigger_fingerprint")
            .and_then(Value::as_str),
    )
    .bind(
        params
            .input_json
            .get("deterministic_campaign_rule")
            .and_then(Value::as_str),
    )
    .bind(params.input_json)
    .bind(params.avatar_file_id)
    .bind(params.avatar_file_unique_id)
    .bind(params.review_threshold)
    .execute(&mut **tx)
    .await?;
    Ok(updated.rows_affected() == 1)
}

#[allow(dead_code)] // Используется PostgreSQL migration/integration harness.
pub async fn enqueue_new_user_audit_job(
    pool: &PgPool,
    params: NewUserAuditJobParams<'_>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    enqueue_new_user_audit_job_in_transaction(&mut tx, params).await?;
    record_new_user_audit_snapshot_in_transaction(&mut tx, params).await?;
    tx.commit().await?;
    Ok(())
}

/// Upsert-ит job в уже открытой транзакции.
///
/// Unified flow берёт строки в одном порядке: `job → audit → review`.
/// Ревизия снимка записывается отдельным шагом после сохранения audit baseline.
pub async fn enqueue_new_user_audit_job_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    params: NewUserAuditJobParams<'_>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into new_user_audit_jobs
            (
                chat_id, telegram_user_id, snapshot_hash, prompt_version, input_json,
                avatar_file_id, avatar_file_unique_id, review_threshold, materialization_version
            )
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        on conflict (chat_id, telegram_user_id, snapshot_hash, prompt_version)
        do update set
            status = case
                when (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then 'pending'
                else new_user_audit_jobs.status
            end,
            generation_fence = new_user_audit_jobs.generation_fence + case when
                (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then 1
                else 0
            end,
            attempts = case
                when (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then 0
                else new_user_audit_jobs.attempts
            end,
            next_attempt_at = case
                when (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then now()
                else new_user_audit_jobs.next_attempt_at
            end,
            processing_started_at = case
                when (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then null
                else new_user_audit_jobs.processing_started_at
            end,
            lease_expires_at = case
                when (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then null
                else new_user_audit_jobs.lease_expires_at
            end,
            error_kind = case
                when (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then null
                else new_user_audit_jobs.error_kind
            end,
            assessment_json = case
                when (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then null
                else new_user_audit_jobs.assessment_json
            end,
            provider = case
                when (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then null
                else new_user_audit_jobs.provider
            end,
            model = case
                when (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then null
                else new_user_audit_jobs.model
            end,
            completed_at = case
                when (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then null
                else new_user_audit_jobs.completed_at
            end,
            input_json = case
                when excluded.input_json ->> 'deterministic_campaign_rule' is null
                  and new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is not null
                then jsonb_set(
                    excluded.input_json,
                    '{deterministic_campaign_rule}',
                    new_user_audit_jobs.input_json -> 'deterministic_campaign_rule',
                    true
                )
                else excluded.input_json
            end,
            avatar_file_id = excluded.avatar_file_id,
            avatar_file_unique_id = excluded.avatar_file_unique_id,
            review_threshold = excluded.review_threshold,
            materialization_version = $9,
            materialization_status = case
                when new_user_audit_jobs.materialization_version is distinct from $9
                  or (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then 'pending'
                else new_user_audit_jobs.materialization_status
            end,
            materialization_fence = new_user_audit_jobs.materialization_fence + case when
                new_user_audit_jobs.materialization_version is distinct from $9
                  or (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then 1 else 0 end,
            materialization_attempts = case
                when new_user_audit_jobs.materialization_version is distinct from $9
                  or (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then 0
                else new_user_audit_jobs.materialization_attempts
            end,
            materialization_next_attempt_at = case
                when new_user_audit_jobs.materialization_version is distinct from $9
                  or (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then now()
                else new_user_audit_jobs.materialization_next_attempt_at
            end,
            materialization_processing_started_at = case
                when new_user_audit_jobs.materialization_version is distinct from $9
                  or (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then null
                else new_user_audit_jobs.materialization_processing_started_at
            end,
            materialization_lease_expires_at = case
                when new_user_audit_jobs.materialization_version is distinct from $9
                  or (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then null
                else new_user_audit_jobs.materialization_lease_expires_at
            end,
            materialization_error_kind = case
                when new_user_audit_jobs.materialization_version is distinct from $9
                  or (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then null
                else new_user_audit_jobs.materialization_error_kind
            end,
            materialized_at = case
                when new_user_audit_jobs.materialization_version is distinct from $9
                  or (new_user_audit_jobs.input_json -> 'text') is distinct from (excluded.input_json -> 'text')
                  or new_user_audit_jobs.input_json ->> 'audit_trigger_fingerprint' is distinct from excluded.input_json ->> 'audit_trigger_fingerprint'
                  or new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule' is distinct from coalesce(excluded.input_json ->> 'deterministic_campaign_rule', new_user_audit_jobs.input_json ->> 'deterministic_campaign_rule')
                  or new_user_audit_jobs.error_kind = 'audit_not_required' then null
                else new_user_audit_jobs.materialized_at
            end,
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
    .bind(params.review_threshold)
    .bind(CURRENT_MATERIALIZATION_VERSION)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Reopens only the saved-assessment materializer when its rules version is
/// obsolete. This never schedules generation or interrupts current retries.
pub async fn reopen_obsolete_materialization_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    params: NewUserAuditJobParams<'_>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update new_user_audit_jobs
        set materialization_version = $5,
            materialization_status = 'pending',
            materialization_attempts = 0,
            materialization_fence = materialization_fence + 1,
            materialization_next_attempt_at = now(),
            materialization_processing_started_at = null,
            materialization_lease_expires_at = null,
            materialization_error_kind = null,
            materialized_at = null,
            updated_at = now()
        where chat_id = $1 and telegram_user_id = $2
          and snapshot_hash = $3 and prompt_version = $4
          and status = 'succeeded' and assessment_json is not null
          and materialization_version is distinct from $5
        "#,
    )
    .bind(params.chat_id)
    .bind(params.telegram_user_id)
    .bind(params.snapshot_hash)
    .bind(params.prompt_version)
    .bind(CURRENT_MATERIALIZATION_VERSION)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Фиксирует ревизию после job upsert-а и audit baseline, не меняя порядок блокировок.
pub async fn record_new_user_audit_snapshot_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    params: NewUserAuditJobParams<'_>,
) -> anyhow::Result<()> {
    let generation: Option<i64> = sqlx::query_scalar(
        "update telegram_new_user_profile_audits set unified_audit_snapshot_hash = $3, unified_audit_generation = unified_audit_generation + 1 where chat_id = $1 and telegram_user_id = $2 returning unified_audit_generation",
    )
    .bind(params.chat_id)
    .bind(params.telegram_user_id)
    .bind(params.snapshot_hash)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(generation) = generation {
        sqlx::query(
            "update new_user_audit_jobs set audit_generation = $5 where chat_id = $1 and telegram_user_id = $2 and snapshot_hash = $3 and prompt_version = $4",
        )
        .bind(params.chat_id)
        .bind(params.telegram_user_id)
        .bind(params.snapshot_hash)
        .bind(params.prompt_version)
        .bind(generation)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Prevents obsolete generation jobs from making LLM requests after the
/// latest profile snapshot has been classified as local-only.
pub async fn supersede_unneeded_new_user_audit_jobs_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    chat_id: i64,
    telegram_user_id: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update new_user_audit_jobs
        set status = 'failed', error_kind = 'audit_not_required', completed_at = now(),
            processing_started_at = null, lease_expires_at = null, updated_at = now()
        where chat_id = $1 and telegram_user_id = $2
          and status in ('pending', 'retry_wait', 'processing')
          and assessment_json is null
        "#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Creates or refreshes the moderator queue row inside the caller's transaction.
pub async fn upsert_spam_review_request_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    chat_id: i64,
    telegram_user_id: i64,
    risk_score: i32,
    review_threshold: i32,
    risk_signals: &Value,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into spam_review_requests
            (chat_id, telegram_user_id, risk_score, review_threshold, risk_signals)
        values ($1, $2, $3, $4, $5)
        on conflict (chat_id, telegram_user_id) do update
        set risk_score = excluded.risk_score,
            review_threshold = excluded.review_threshold,
            risk_signals = excluded.risk_signals,
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
    .bind(chat_id)
    .bind(telegram_user_id)
    .bind(risk_score.clamp(0, 100))
    .bind(review_threshold.clamp(0, 100))
    .bind(risk_signals)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Uses the saved combined score and signals as the source of truth for local review.
pub async fn sync_spam_review_from_audit_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    chat_id: i64,
    telegram_user_id: i64,
    review_threshold: i32,
) -> anyhow::Result<()> {
    let (risk_score, risk_signals): (i32, Value) = sqlx::query_as(
        "select risk_score, risk_signal_breakdown from telegram_new_user_profile_audits where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .fetch_one(&mut **tx)
    .await?;
    if risk_score >= review_threshold {
        return upsert_spam_review_request_in_transaction(
            tx,
            chat_id,
            telegram_user_id,
            risk_score,
            review_threshold,
            &risk_signals,
        )
        .await;
    }

    // A lower local score must not create a new queue item, but should refresh
    // an existing pending row so an older high-risk notification cannot win a
    // race with the newer snapshot.
    sqlx::query(
        r#"
        update spam_review_requests
        set risk_score = $3, review_threshold = $4, risk_signals = $5
        where chat_id = $1 and telegram_user_id = $2 and status = 'pending'
        "#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .bind(risk_score.clamp(0, 100))
    .bind(review_threshold.clamp(0, 100))
    .bind(risk_signals)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Claims either a fresh generation or a stored assessment awaiting materialization.
pub async fn claim_next_new_user_audit_job(
    pool: &PgPool,
) -> anyhow::Result<Option<NewUserAuditJob>> {
    let row = sqlx::query(
        r#"
        with candidate as (
            select id,
                   assessment_json is not null as is_materialization_replay,
                   case when assessment_json is not null
                        then materialization_next_attempt_at else next_attempt_at end as ready_at
            from new_user_audit_jobs
            where (
                    assessment_json is null
                    and (
                        (status in ('pending', 'retry_wait') and next_attempt_at <= now())
                        or (status = 'processing' and lease_expires_at <= now())
                    )
                  )
               or (
                    status = 'succeeded'
                    and assessment_json is not null
                    and materialization_version = $2
                    and (
                        (materialization_status in ('pending', 'retry_wait')
                         and materialization_next_attempt_at <= now())
                        or (materialization_status = 'processing'
                            and materialization_lease_expires_at <= now())
                    )
                  )
            order by (assessment_json is not null), ready_at, id
            for update skip locked
            limit 1
        )
        update new_user_audit_jobs job
        set status = case when candidate.is_materialization_replay then job.status else 'processing' end,
            attempts = case when candidate.is_materialization_replay then job.attempts else job.attempts + 1 end,
            generation_fence = case when candidate.is_materialization_replay then job.generation_fence else job.generation_fence + 1 end,
            processing_started_at = case when candidate.is_materialization_replay then job.processing_started_at else now() end,
            lease_expires_at = case when candidate.is_materialization_replay then job.lease_expires_at else now() + ($1 * interval '1 second') end,
            materialization_status = case when candidate.is_materialization_replay then 'processing' else job.materialization_status end,
            materialization_attempts = case when candidate.is_materialization_replay then job.materialization_attempts + 1 else job.materialization_attempts end,
            materialization_fence = case when candidate.is_materialization_replay then job.materialization_fence + 1 else job.materialization_fence end,
            materialization_processing_started_at = case when candidate.is_materialization_replay then now() else job.materialization_processing_started_at end,
            materialization_lease_expires_at = case when candidate.is_materialization_replay then now() + ($1 * interval '1 second') else job.materialization_lease_expires_at end,
            updated_at = now()
        from candidate
        where job.id = candidate.id
        returning job.id, job.chat_id, job.telegram_user_id, job.snapshot_hash,
                  job.prompt_version, job.input_json, job.avatar_file_id,
                  job.avatar_file_unique_id, job.assessment_json, job.review_threshold, job.attempts,
                  job.materialization_attempts, job.generation_fence, job.materialization_fence,
                  job.audit_generation, candidate.is_materialization_replay
        "#,
    )
    .bind(EXTERNAL_REQUEST_LEASE.seconds())
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
        review_threshold: row.get("review_threshold"),
        attempts: row.get("attempts"),
        materialization_attempts: row.get("materialization_attempts"),
        generation_fence: row.get("generation_fence"),
        materialization_fence: row.get("materialization_fence"),
        audit_generation: row.get("audit_generation"),
        is_materialization_replay: row.get("is_materialization_replay"),
    }))
}

pub struct NewUserAuditOutcome<'a> {
    pub assessment_json: &'a Value,
    pub provider: &'a str,
    pub model: &'a str,
}

/// Persists the model assessment and opens the materialization stage.
pub async fn finalize_new_user_audit_job(
    pool: &PgPool,
    job: &NewUserAuditJob,
    outcome: NewUserAuditOutcome<'_>,
) -> anyhow::Result<CasResult> {
    finalize_new_user_audit_generation(pool, job, outcome).await
}

/// Durable boundary between LLM generation and materialization.
///
/// Once this CAS succeeds, the worker must only use the stored assessment through
/// the materialization replay lifecycle. In particular, SQL or embedding failures
/// after this point must never reopen LLM generation.
async fn finalize_new_user_audit_generation(
    pool: &PgPool,
    job: &NewUserAuditJob,
    outcome: NewUserAuditOutcome<'_>,
) -> anyhow::Result<CasResult> {
    for retry in 0..=GENERATION_FINALIZATION_SQL_RETRIES {
        let update = sqlx::query(
            r#"
            update new_user_audit_jobs
            set status = 'succeeded', assessment_json = $3, provider = $4, model = $5,
                materialization_version = $6, completed_at = now(), error_kind = null,
                processing_started_at = null, lease_expires_at = null,
                materialization_status = 'pending', materialization_attempts = 0,
                materialization_next_attempt_at = now(),
                materialization_processing_started_at = null,
                materialization_lease_expires_at = null, materialization_error_kind = null,
                materialized_at = null,
                updated_at = now()
            where id = $1 and attempts = $2 and generation_fence = $7 and status = 'processing'
              and lease_expires_at > now()
            "#,
        )
        .bind(job.id)
        .bind(job.attempts)
        .bind(outcome.assessment_json)
        .bind(outcome.provider)
        .bind(outcome.model)
        .bind(CURRENT_MATERIALIZATION_VERSION)
        .bind(job.generation_fence)
        .execute(pool)
        .await;

        match update {
            Ok(update) => return CasResult::from_rows_affected(update.rows_affected()),
            Err(error)
                if should_retry_generation_finalization(
                    retry,
                    is_transient_generation_finalization_error(&error),
                ) =>
            {
                tokio::time::sleep(GENERATION_FINALIZATION_RETRY_DELAY).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!("generation finalization retry loop always returns")
}

fn should_retry_generation_finalization(retry: u32, is_transient_sql_error: bool) -> bool {
    retry < GENERATION_FINALIZATION_SQL_RETRIES && is_transient_sql_error
}

fn is_transient_generation_finalization_error(error: &sqlx::Error) -> bool {
    is_transient_sqlx_error(error)
}

pub(super) fn is_transient_sqlx_error(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) | sqlx::Error::PoolTimedOut => true,
        sqlx::Error::Database(database_error) => database_error
            .code()
            .is_some_and(|code| is_transient_postgres_sqlstate(code.as_ref())),
        _ => false,
    }
}

fn is_transient_postgres_sqlstate(code: &str) -> bool {
    matches!(
        code,
        // Transaction conflicts and explicitly cancelled/locked statements.
        "40001" | "40P01" | "55P03" | "57014"
            // Connection failures while PostgreSQL is unavailable or restarting.
            | "08000" | "08001" | "08003" | "08004" | "08006" | "08007"
            // PostgreSQL shutdown/restart and temporary connection-capacity pressure.
            | "57P01" | "57P02" | "57P03" | "53300"
    )
}

/// Применяет current-score к baseline только пока canonical snapshot не изменился.
async fn materialize_new_user_audit_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    job: &NewUserAuditJob,
    components: &ScoreComponents,
    embedding_model: &str,
    deterministic_spam_label: Option<&crate::features::labels::SpamLabel>,
) -> anyhow::Result<CasResult> {
    // Keep the database's non-negative score-component constraints authoritative,
    // even if a future scorer accidentally emits a negative component.
    let baseline_score = components.baseline_score.clamp(0, 100);
    let first_message_score = components.first_message_score.clamp(0, 100);
    let avatar_score = components.avatar_score.clamp(0, 100);
    let personal_channel_score = components.personal_channel_score.clamp(0, 100);
    let external_score = components.external_score.clamp(0, 100);
    if baseline_score != components.baseline_score
        || first_message_score != components.first_message_score
        || avatar_score != components.avatar_score
        || personal_channel_score != components.personal_channel_score
        || external_score != components.external_score
    {
        tracing::warn!(
            job_id = job.id,
            chat_id = job.chat_id,
            telegram_user_id = job.telegram_user_id,
            baseline_score = components.baseline_score,
            first_message_score = components.first_message_score,
            avatar_score = components.avatar_score,
            personal_channel_score = components.personal_channel_score,
            external_score = components.external_score,
            "clamping out-of-range new user audit score component before materialization"
        );
    }
    let final_score = baseline_score
        .saturating_add(first_message_score)
        .saturating_add(avatar_score)
        .saturating_add(personal_channel_score)
        .saturating_add(external_score)
        .clamp(0, 100);
    let final_signals = components.final_signals();
    let audit_update = sqlx::query(
        r#"
        update telegram_new_user_profile_audits
        set risk_baseline_score = $3, risk_baseline_signals = $4,
            risk_first_message_score = $5, risk_first_message_signals = $6,
            risk_avatar_score = $7, risk_avatar_signals = $8,
            risk_personal_channel_score = $9, risk_personal_channel_signals = $10,
            risk_external_score = $11, risk_external_signals = $12,
            risk_score = $13, risk_level = $14, risk_signal_breakdown = $15,
            first_message_embedding_gemma2 = case
                when $18 is null then first_message_embedding_gemma2
                else $18::vector
            end,
            first_message_embedding_gemma2_model = case
                when $18 is null then first_message_embedding_gemma2_model
                else $19
            end
        where chat_id = $1 and telegram_user_id = $2
          and unified_audit_snapshot_hash = $16
          and unified_audit_generation = $17
        "#,
    )
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .bind(baseline_score)
    .bind(&components.baseline_signals)
    .bind(first_message_score)
    .bind(&components.first_message_signals)
    .bind(avatar_score)
    .bind(&components.avatar_signals)
    .bind(personal_channel_score)
    .bind(&components.personal_channel_signals)
    .bind(external_score)
    .bind(&components.external_signals)
    .bind(final_score)
    .bind(components.final_level())
    .bind(&final_signals)
    .bind(&job.snapshot_hash)
    .bind(job.audit_generation)
    .bind(components.first_message_embedding.clone())
    .bind(embedding_model)
    .execute(&mut **tx)
    .await?;
    if audit_update.rows_affected() == 0 {
        let stale = sqlx::query(
            r#"update new_user_audit_jobs
               set materialization_status = case when exists (
                       select 1 from telegram_new_user_profile_audits a
                       where a.chat_id = new_user_audit_jobs.chat_id
                         and a.telegram_user_id = new_user_audit_jobs.telegram_user_id
                         and a.unified_audit_snapshot_hash = new_user_audit_jobs.snapshot_hash
                   ) then 'retry_wait' else 'stale' end,
                   materialization_next_attempt_at = now() + interval '5 seconds',
                   materialized_at = now(),
                   materialization_error_kind = case when exists (
                       select 1 from telegram_new_user_profile_audits a
                       where a.chat_id = new_user_audit_jobs.chat_id
                         and a.telegram_user_id = new_user_audit_jobs.telegram_user_id
                         and a.unified_audit_snapshot_hash = new_user_audit_jobs.snapshot_hash
                   ) then 'snapshot_generation_changed' else 'snapshot_stale' end,
                   materialization_processing_started_at = null,
                   materialization_lease_expires_at = null
               where id = $1 and (not $4 or (status = 'succeeded'
                 and materialization_status = 'processing'
                 and materialization_attempts = $2
                 and materialization_version = $3
                 and materialization_fence = $5))"#,
        )
        .bind(job.id)
        .bind(job.materialization_attempts)
        .bind(CURRENT_MATERIALIZATION_VERSION)
        .bind(job.is_materialization_replay)
        .bind(job.materialization_fence)
        .execute(&mut **tx)
        .await?;
        let stale_result = CasResult::from_rows_affected(stale.rows_affected())?;
        return Ok(if stale_result == CasResult::Applied {
            CasResult::Stale
        } else {
            stale_result
        });
    }

    if let Some(label) = deterministic_spam_label {
        let current_label_state: Option<(bool, Option<String>)> = sqlx::query_as(
            r#"select coalesce(u.is_spammer, false), (
                   select e.label from spam_label_events e
                   where e.chat_id = u.chat_id and e.telegram_user_id = u.telegram_user_id
                   order by e.created_at desc, e.id desc limit 1
               )
               from telegram_chat_users u
               where u.chat_id = $1 and u.telegram_user_id = $2
               for update of u"#,
        )
        .bind(job.chat_id)
        .bind(job.telegram_user_id)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some((false, latest_label)) = current_label_state
            && latest_label.as_deref() != Some("not_spam")
        {
            crate::features::labels::record_spam_in_transaction(
                tx,
                job.chat_id,
                job.telegram_user_id,
                label,
                false,
            )
            .await?;
        }
    }

    upsert_spam_review_request_in_transaction(
        tx,
        job.chat_id,
        job.telegram_user_id,
        final_score,
        job.review_threshold,
        &final_signals,
    )
    .await?;
    let finalized = sqlx::query(
        "update new_user_audit_jobs set materialization_status = 'succeeded', materialized_at = now(), materialization_error_kind = null, materialization_processing_started_at = null, materialization_lease_expires_at = null where id = $1 and (not $4 or (status = 'succeeded' and materialization_status = 'processing' and materialization_attempts = $2 and materialization_version = $3 and materialization_fence = $5))",
    )
    .bind(job.id)
    .bind(job.materialization_attempts)
    .bind(CURRENT_MATERIALIZATION_VERSION)
    .bind(job.is_materialization_replay)
    .bind(job.materialization_fence)
    .execute(&mut **tx)
    .await?;
    CasResult::from_rows_affected(finalized.rows_affected())?;
    Ok(CasResult::Applied)
}

/// Завершает lease успешной job и materializes уже сохранённый assessment.
/// Assessment не перезаписывается, поэтому этот путь никогда не инициирует LLM generation.
pub async fn materialize_new_user_audit_job(
    pool: &PgPool,
    job: &NewUserAuditJob,
    components: &ScoreComponents,
    embedding_model: &str,
    deterministic_spam_label: Option<&crate::features::labels::SpamLabel>,
) -> anyhow::Result<CasResult> {
    let mut tx = pool.begin().await?;
    let update = sqlx::query(
        "update new_user_audit_jobs set updated_at = now() where id = $1 and status = 'succeeded' and assessment_json is not null and materialization_status = 'processing' and materialization_attempts = $2 and materialization_version = $3 and materialization_fence = $4",
    )
    .bind(job.id)
    .bind(job.materialization_attempts)
    .bind(CURRENT_MATERIALIZATION_VERSION)
    .bind(job.materialization_fence)
    .execute(&mut *tx)
    .await?;
    let result = CasResult::from_rows_affected(update.rows_affected())?;
    if result == CasResult::LeaseLost {
        tx.rollback().await?;
        return Ok(result);
    }
    let result = materialize_new_user_audit_in_transaction(
        &mut tx,
        job,
        components,
        embedding_model,
        deterministic_spam_label,
    )
    .await?;
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
        "update new_user_audit_jobs set materialization_status = 'stale', materialized_at = now(), materialization_error_kind = $3, materialization_processing_started_at = null, materialization_lease_expires_at = null, updated_at = now() where id = $1 and status = 'succeeded' and assessment_json is not null and materialization_status = 'processing' and materialization_attempts = $2 and materialization_version = $4 and materialization_fence = $5",
    )
    .bind(job.id)
    .bind(job.materialization_attempts)
    .bind(error_kind)
    .bind(CURRENT_MATERIALIZATION_VERSION)
    .bind(job.materialization_fence)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(update.rows_affected())
}

/// Schedules a replay retry without reopening or altering the successful generation.
pub async fn mark_new_user_audit_materialization_retry(
    pool: &PgPool,
    job: &NewUserAuditJob,
    error_kind: &str,
) -> anyhow::Result<CasResult> {
    let Some(delay_seconds) =
        MATERIALIZATION_RETRY.delay_seconds(job.materialization_attempts, None)
    else {
        return mark_new_user_audit_materialization_stale(pool, job, "retry_exhausted").await;
    };
    let update = sqlx::query(
        "update new_user_audit_jobs set materialization_status = 'retry_wait', materialization_error_kind = $3, materialization_next_attempt_at = now() + ($4 * interval '1 second'), materialization_processing_started_at = null, materialization_lease_expires_at = null, updated_at = now() where id = $1 and status = 'succeeded' and assessment_json is not null and materialization_status = 'processing' and materialization_attempts = $2 and materialization_version = $5 and materialization_fence = $6",
    )
    .bind(job.id)
    .bind(job.materialization_attempts)
    .bind(error_kind)
    .bind(delay_seconds)
    .bind(CURRENT_MATERIALIZATION_VERSION)
    .bind(job.materialization_fence)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(update.rows_affected())
}

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
        where id = $1 and attempts = $2 and generation_fence = $5 and status = 'processing'
        "#,
    )
    .bind(job.id)
    .bind(job.attempts)
    .bind(error_kind)
    .bind(delay_seconds)
    .bind(job.generation_fence)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(update.rows_affected())
}

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
        where id = $1 and attempts = $2 and generation_fence = $4 and status = 'processing'
        "#,
    )
    .bind(job.id)
    .bind(job.attempts)
    .bind(error_kind)
    .bind(job.generation_fence)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(update.rows_affected())
}

/// Отозванный источник/изменившаяся policy заново открывает generation.
/// Fence сохраняет монотонность: старый local worker уже не финализирует запись.
pub async fn restart_revoked_local_assessment(
    pool: &PgPool,
    job: &NewUserAuditJob,
) -> anyhow::Result<CasResult> {
    let result=sqlx::query(
        r#"update new_user_audit_jobs set status='retry_wait', assessment_json=null,
           provider=null,model=null,attempts=0,generation_fence=generation_fence+1,
           next_attempt_at=now(),processing_started_at=null,lease_expires_at=null,completed_at=null,
           error_kind='local_evidence_revoked',materialization_status='pending',
           materialization_fence=materialization_fence+1,
           materialization_processing_started_at=null,materialization_lease_expires_at=null,updated_at=now()
           where id=$1 and status='succeeded' and materialization_status='processing'
             and materialization_fence=$2 and generation_fence=$3
             and materialization_version=$4 and materialization_lease_expires_at>now()
             and assessment_json #>> '{_local_cascade,applied}'='true'
             and assessment_json #>> '{_local_cascade,version}'=$5"#
    ).bind(job.id).bind(job.materialization_fence).bind(job.generation_fence)
        .bind(CURRENT_MATERIALIZATION_VERSION).bind(super::local_cascade::VERSION).execute(pool).await?;
    CasResult::from_rows_affected(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_generation_finalization_retries_stay_local_and_bounded() {
        assert!(should_retry_generation_finalization(0, true));
        assert!(should_retry_generation_finalization(
            GENERATION_FINALIZATION_SQL_RETRIES - 1,
            true
        ));
        assert!(!should_retry_generation_finalization(
            GENERATION_FINALIZATION_SQL_RETRIES,
            true
        ));
        assert!(!should_retry_generation_finalization(0, false));
    }

    #[test]
    fn transient_generation_finalization_recognizes_sqlx_transport_errors() {
        assert!(is_transient_generation_finalization_error(
            &sqlx::Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset),)
        ));
        assert!(is_transient_generation_finalization_error(
            &sqlx::Error::Tls(Box::new(std::io::Error::from(
                std::io::ErrorKind::ConnectionAborted,
            )),)
        ));
        assert!(is_transient_generation_finalization_error(
            &sqlx::Error::PoolTimedOut
        ));
        assert!(!is_transient_generation_finalization_error(
            &sqlx::Error::PoolClosed
        ));
    }

    #[test]
    fn transient_postgres_sqlstates_are_limited_to_retryable_conditions() {
        for code in [
            "40001", "40P01", "55P03", "57014", "08006", "57P01", "53300",
        ] {
            assert!(is_transient_postgres_sqlstate(code), "{code}");
        }
        for code in ["23505", "23514", "42501", "42601", "08P01"] {
            assert!(!is_transient_postgres_sqlstate(code), "{code}");
        }
    }
}
