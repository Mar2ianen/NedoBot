#![allow(dead_code)]

use sqlx::PgPool;

use crate::features::voice::cleanup::CleanupResult;
use crate::features::voice::types::{AsrTranscript, VoiceMedia};

pub async fn create_voice_job(pool: &PgPool, media: &VoiceMedia) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query_as::<_, (i64,)>(
        r#"
        insert into voice_transcription_jobs
            (
                chat_id, message_id, user_id, file_id, file_unique_id,
                media_kind, duration_sec, file_size, mime_type
            )
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        on conflict (chat_id, message_id) do nothing
        returning id
        "#,
    )
    .bind(media.chat_id)
    .bind(media.message_id)
    .bind(media.user_id)
    .bind(&media.file_id)
    .bind(&media.file_unique_id)
    .bind(media.kind.as_str())
    .bind(media.duration_sec.map(|value| value as i32))
    .bind(media.file_size.map(|value| value as i64))
    .bind(&media.mime_type)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(id,)| id))
}

pub async fn mark_voice_job_status(pool: &PgPool, job_id: i64, status: &str) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update voice_transcription_jobs
        set status = $2, updated_at = now()
        where id = $1
        "#,
    )
    .bind(job_id)
    .bind(status)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn mark_voice_job_failed(pool: &PgPool, job_id: i64, error: &str) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update voice_transcription_jobs
        set status = 'failed', error = $2, updated_at = now()
        where id = $1
        "#,
    )
    .bind(job_id)
    .bind(error)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn save_asr_result(
    pool: &PgPool,
    job_id: i64,
    transcript: &AsrTranscript,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update voice_transcription_jobs
        set asr_provider = $2,
            asr_model = $3,
            asr_request_id = $4,
            raw_transcript = $5,
            segments_json = $6,
            raw_asr_json = $7,
            updated_at = now()
        where id = $1
        "#,
    )
    .bind(job_id)
    .bind(&transcript.provider)
    .bind(&transcript.model)
    .bind(&transcript.request_id)
    .bind(&transcript.text)
    .bind(serde_json::to_value(&transcript.segments)?)
    .bind(&transcript.raw_json)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn save_voice_result(
    pool: &PgPool,
    job_id: i64,
    result: &CleanupResult,
    final_html: &str,
    full_text_file_id: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update voice_transcription_jobs
        set status = 'sent',
            cleanup_provider = $2,
            cleanup_model = $3,
            cleaned_text = $4,
            render_mode = $5,
            chapters_json = $6,
            final_html = $7,
            full_text_file_id = $8,
            updated_at = now()
        where id = $1
        "#,
    )
    .bind(job_id)
    .bind(&result.provider)
    .bind(&result.model)
    .bind(&result.transcript.text)
    .bind(result.transcript.mode.as_str())
    .bind(serde_json::to_value(&result.transcript.chapters)?)
    .bind(final_html)
    .bind(full_text_file_id)
    .execute(pool)
    .await?;

    Ok(())
}
