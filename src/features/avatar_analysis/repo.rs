#![allow(dead_code)] // The worker is introduced after the prompt and classifier slices.

use serde_json::Value;
use sqlx::{PgPool, Row};

const LEASE_SECONDS: i64 = 10 * 60;

#[derive(Debug, Clone)]
pub struct AvatarAnalysisJob {
    pub id: i64,
    pub telegram_user_id: i64,
    pub profile_photo_file_id: String,
    pub profile_photo_file_unique_id: String,
    pub features_snapshot_hash: String,
    pub prompt_version: String,
    pub attempts: i32,
}

pub async fn enqueue_avatar_analysis_job(
    pool: &PgPool,
    telegram_user_id: i64,
    profile_photo_file_id: &str,
    profile_photo_file_unique_id: &str,
    features_snapshot_hash: &str,
    prompt_version: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into avatar_analysis_jobs
            (telegram_user_id, profile_photo_file_id, profile_photo_file_unique_id, features_snapshot_hash, prompt_version)
        values ($1, $2, $3, $4, $5)
        on conflict (telegram_user_id, profile_photo_file_unique_id, features_snapshot_hash, prompt_version)
        do update set
            profile_photo_file_id = excluded.profile_photo_file_id,
            updated_at = now()
        "#,
    )
    .bind(telegram_user_id)
    .bind(profile_photo_file_id)
    .bind(profile_photo_file_unique_id)
    .bind(features_snapshot_hash)
    .bind(prompt_version)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn claim_next_avatar_analysis_job(
    pool: &PgPool,
) -> anyhow::Result<Option<AvatarAnalysisJob>> {
    let row = sqlx::query(
        r#"
        with candidate as (
            select id
            from avatar_analysis_jobs
            where (status in ('pending', 'retry_wait') and next_attempt_at <= now())
               or (status = 'processing' and lease_expires_at <= now())
            order by next_attempt_at, id
            for update skip locked
            limit 1
        )
        update avatar_analysis_jobs job
        set status = 'processing',
            attempts = job.attempts + 1,
            processing_started_at = now(),
            lease_expires_at = now() + ($1 * interval '1 second'),
            updated_at = now()
        from candidate
        where job.id = candidate.id
        returning job.id, job.telegram_user_id, job.profile_photo_file_id,
                  job.profile_photo_file_unique_id, job.features_snapshot_hash,
                  job.prompt_version, job.attempts
        "#,
    )
    .bind(LEASE_SECONDS)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| AvatarAnalysisJob {
        id: row.get("id"),
        telegram_user_id: row.get("telegram_user_id"),
        profile_photo_file_id: row.get("profile_photo_file_id"),
        profile_photo_file_unique_id: row.get("profile_photo_file_unique_id"),
        features_snapshot_hash: row.get("features_snapshot_hash"),
        prompt_version: row.get("prompt_version"),
        attempts: row.get("attempts"),
    }))
}

pub async fn mark_avatar_analysis_succeeded(
    pool: &PgPool,
    job: &AvatarAnalysisJob,
    provider: &str,
    model: &str,
    input_hash: &str,
    observation: &Value,
    assessment: &Value,
    response: &Value,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        insert into avatar_image_analyses
            (profile_photo_file_unique_id, prompt_version, provider, model, input_hash, observation_json, response_json)
        values ($1, $2, $3, $4, $5, $6, $7)
        on conflict (profile_photo_file_unique_id, prompt_version) do update set
            provider = excluded.provider, model = excluded.model, input_hash = excluded.input_hash,
            observation_json = excluded.observation_json, response_json = excluded.response_json, analyzed_at = now()
        "#,
    )
    .bind(&job.profile_photo_file_unique_id)
    .bind(&job.prompt_version)
    .bind(provider)
    .bind(model)
    .bind(input_hash)
    .bind(observation)
    .bind(response)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        insert into avatar_profile_assessments
            (telegram_user_id, profile_photo_file_unique_id, features_snapshot_hash, prompt_version, provider, model, input_hash, assessment_json, response_json)
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        on conflict (telegram_user_id, profile_photo_file_unique_id, features_snapshot_hash, prompt_version) do update set
            provider = excluded.provider, model = excluded.model, input_hash = excluded.input_hash,
            assessment_json = excluded.assessment_json, response_json = excluded.response_json, analyzed_at = now()
        "#,
    )
    .bind(job.telegram_user_id)
    .bind(&job.profile_photo_file_unique_id)
    .bind(&job.features_snapshot_hash)
    .bind(&job.prompt_version)
    .bind(provider)
    .bind(model)
    .bind(input_hash)
    .bind(assessment)
    .bind(response)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        update avatar_analysis_jobs
        set status = 'succeeded', provider = $2, model = $3, error_kind = null,
            lease_expires_at = null, updated_at = now()
        where id = $1 and status = 'processing'
        "#,
    )
    .bind(job.id)
    .bind(provider)
    .bind(model)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn mark_avatar_analysis_failed(
    pool: &PgPool,
    job: &AvatarAnalysisJob,
    error_kind: &str,
    retry_after_seconds: Option<i64>,
) -> anyhow::Result<()> {
    let delay_seconds = retry_after_seconds.unwrap_or(0).max(0);
    let (status, next_delay) = match job.attempts {
        1 => ("retry_wait", delay_seconds.max(5 * 60)),
        2 => ("retry_wait", delay_seconds.max(24 * 60 * 60)),
        _ => ("failed", 0),
    };
    sqlx::query(
        r#"
        update avatar_analysis_jobs
        set status = $2, error_kind = $3,
            next_attempt_at = now() + ($4 * interval '1 second'),
            lease_expires_at = null, updated_at = now()
        where id = $1 and status = 'processing'
        "#,
    )
    .bind(job.id)
    .bind(status)
    .bind(error_kind)
    .bind(next_delay)
    .execute(pool)
    .await?;
    Ok(())
}
