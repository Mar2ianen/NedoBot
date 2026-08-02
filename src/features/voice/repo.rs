use sqlx::PgPool;

use crate::features::jobs::policy::{VOICE_TRANSCRIPTION_LEASE, VOICE_TRANSCRIPTION_RETRY};
use crate::features::voice::cleanup::CleanupResult;
use crate::features::voice::types::{AsrTranscript, VoiceMedia, VoiceMediaKind};

#[derive(Debug, Clone)]
pub struct VoiceJob {
    pub id: i64,
    pub chat_id: i64,
    pub message_id: i32,
    pub user_id: Option<i64>,
    pub file_id: String,
    pub file_unique_id: Option<String>,
    pub media_kind: String,
    pub duration_sec: Option<i32>,
    pub file_size: Option<i64>,
    pub mime_type: Option<String>,
    pub attempts: i32,
    pub progress_message_id: Option<i32>,
}

impl VoiceJob {
    pub fn media(&self) -> anyhow::Result<VoiceMedia> {
        let kind = VoiceMediaKind::parse(&self.media_kind)
            .ok_or_else(|| anyhow::anyhow!("unknown voice media kind: {}", self.media_kind))?;
        Ok(VoiceMedia {
            chat_id: self.chat_id,
            message_id: self.message_id,
            user_id: self.user_id,
            kind,
            file_id: self.file_id.clone(),
            file_unique_id: self.file_unique_id.clone(),
            duration_sec: self
                .duration_sec
                .and_then(|value| u32::try_from(value).ok()),
            file_size: self.file_size.and_then(|value| u64::try_from(value).ok()),
            mime_type: self.mime_type.clone(),
        })
    }
}

type VoiceJobRow = (
    i64,
    i64,
    i32,
    Option<i64>,
    String,
    Option<String>,
    String,
    Option<i32>,
    Option<i64>,
    Option<String>,
    i32,
    Option<i32>,
);

fn row_to_job(row: VoiceJobRow) -> VoiceJob {
    let (
        id,
        chat_id,
        message_id,
        user_id,
        file_id,
        file_unique_id,
        media_kind,
        duration_sec,
        file_size,
        mime_type,
        attempts,
        progress_message_id,
    ) = row;
    VoiceJob {
        id,
        chat_id,
        message_id,
        user_id,
        file_id,
        file_unique_id,
        media_kind,
        duration_sec,
        file_size,
        mime_type,
        attempts,
        progress_message_id,
    }
}

const JOB_COLUMNS: &str = "id, chat_id, message_id, user_id, file_id, file_unique_id, media_kind, duration_sec, file_size, mime_type, attempts, progress_message_id";
const JOB_COLUMNS_QUALIFIED: &str = "job.id, job.chat_id, job.message_id, job.user_id, job.file_id, job.file_unique_id, job.media_kind, job.duration_sec, job.file_size, job.mime_type, job.attempts, job.progress_message_id";

pub async fn create_voice_job(pool: &PgPool, media: &VoiceMedia) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query_as::<_, (i64,)>(
        r#"
        insert into voice_transcription_jobs
            (
                chat_id, message_id, user_id, file_id, file_unique_id,
                media_kind, duration_sec, file_size, mime_type
            )
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        on conflict (chat_id, message_id) do update
        set updated_at = now()
        where voice_transcription_jobs.status in ('pending', 'retry_wait')
           or (voice_transcription_jobs.status in ('processing', 'downloading', 'transcribing', 'cleaning')
               and voice_transcription_jobs.lease_expires_at <= now())
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

pub async fn claim_voice_job(pool: &PgPool, job_id: i64) -> anyhow::Result<Option<VoiceJob>> {
    let row = sqlx::query_as::<_, VoiceJobRow>(&format!(
        r#"
        update voice_transcription_jobs
        set status = 'processing',
            attempts = attempts + 1,
            processing_started_at = now(),
            lease_expires_at = now() + ($2 * interval '1 second'),
            error = null,
            error_kind = null,
            updated_at = now()
        where id = $1
          and (
              (status in ('pending', 'retry_wait') and next_attempt_at <= now())
              or (status in ('processing', 'downloading', 'transcribing', 'cleaning')
                  and lease_expires_at <= now())
          )
        returning {JOB_COLUMNS}
        "#,
        JOB_COLUMNS = JOB_COLUMNS
    ))
    .bind(job_id)
    .bind(VOICE_TRANSCRIPTION_LEASE.seconds())
    .fetch_optional(pool)
    .await?;

    Ok(row.map(row_to_job))
}

pub async fn claim_next_voice_job(pool: &PgPool) -> anyhow::Result<Option<VoiceJob>> {
    let row = sqlx::query_as::<_, VoiceJobRow>(&format!(
        r#"
        with candidate as (
            select id
            from voice_transcription_jobs
            where (
                (status in ('pending', 'retry_wait') and next_attempt_at <= now())
                or (status in ('processing', 'downloading', 'transcribing', 'cleaning')
                    and lease_expires_at <= now())
            )
            order by next_attempt_at, id
            for update skip locked
            limit 1
        )
        update voice_transcription_jobs job
        set status = 'processing',
            attempts = job.attempts + 1,
            processing_started_at = now(),
            lease_expires_at = now() + ($1 * interval '1 second'),
            error = null,
            error_kind = null,
            updated_at = now()
        from candidate
        where job.id = candidate.id
        returning {JOB_COLUMNS_QUALIFIED}
        "#,
        JOB_COLUMNS_QUALIFIED = JOB_COLUMNS_QUALIFIED
    ))
    .bind(VOICE_TRANSCRIPTION_LEASE.seconds())
    .fetch_optional(pool)
    .await?;

    Ok(row.map(row_to_job))
}

pub async fn save_progress_message(
    pool: &PgPool,
    job: &VoiceJob,
    progress_message_id: i32,
) -> anyhow::Result<bool> {
    let query_result = sqlx::query(
        r#"
        update voice_transcription_jobs
        set progress_message_id = $2,
            updated_at = now()
        where id = $1 and attempts = $3 and status = 'processing'
          and lease_expires_at > now()
        "#,
    )
    .bind(job.id)
    .bind(progress_message_id)
    .bind(job.attempts)
    .execute(pool)
    .await?;
    Ok(query_result.rows_affected() == 1)
}

pub async fn mark_voice_job_phase(
    pool: &PgPool,
    job: &VoiceJob,
    status: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"
        update voice_transcription_jobs
        set status = $2,
            lease_expires_at = now() + ($3 * interval '1 second'),
            updated_at = now()
        where id = $1 and attempts = $4
          and status in ('processing', 'downloading', 'transcribing', 'cleaning')
          and lease_expires_at > now()
        "#,
    )
    .bind(job.id)
    .bind(status)
    .bind(VOICE_TRANSCRIPTION_LEASE.seconds())
    .bind(job.attempts)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn mark_voice_job_failed(
    pool: &PgPool,
    job: &VoiceJob,
    error_kind: &str,
    error: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"
        update voice_transcription_jobs
        set status = 'failed', error_kind = $2, error = $3,
            processing_started_at = null, lease_expires_at = null, updated_at = now()
        where id = $1 and attempts = $4 and status <> 'sent'
        "#,
    )
    .bind(job.id)
    .bind(error_kind)
    .bind(error)
    .bind(job.attempts)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn mark_voice_job_retry_or_failed(
    pool: &PgPool,
    job: &VoiceJob,
    error_kind: &str,
    error: &str,
) -> anyhow::Result<()> {
    let next_delay = VOICE_TRANSCRIPTION_RETRY.delay_seconds(job.attempts, None);
    if let Some(delay) = next_delay {
        sqlx::query(
            r#"
            update voice_transcription_jobs
            set status = 'retry_wait', error_kind = $2, error = $3,
                next_attempt_at = now() + ($4 * interval '1 second'),
                processing_started_at = null, lease_expires_at = null, updated_at = now()
            where id = $1 and attempts = $5 and status <> 'sent'
            "#,
        )
        .bind(job.id)
        .bind(error_kind)
        .bind(error)
        .bind(delay)
        .bind(job.attempts)
        .execute(pool)
        .await?;
    } else {
        mark_voice_job_failed(pool, job, error_kind, error).await?;
    }
    Ok(())
}

pub async fn mark_voice_job_skipped(
    pool: &PgPool,
    job: &VoiceJob,
    error: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"
        update voice_transcription_jobs
        set status = 'skipped', error_kind = 'no_speech', error = $2,
            processing_started_at = null, lease_expires_at = null, updated_at = now()
        where id = $1 and attempts = $3
        "#,
    )
    .bind(job.id)
    .bind(error)
    .bind(job.attempts)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn save_asr_result(
    pool: &PgPool,
    job: &VoiceJob,
    transcript: &AsrTranscript,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"
        update voice_transcription_jobs
        set asr_provider = $2,
            asr_model = $3,
            asr_request_id = $4,
            raw_transcript = $5,
            segments_json = $6,
            raw_asr_json = $7,
            updated_at = now()
        where id = $1 and attempts = $8 and status in ('transcribing', 'cleaning')
          and lease_expires_at > now()
        "#,
    )
    .bind(job.id)
    .bind(&transcript.provider)
    .bind(&transcript.model)
    .bind(&transcript.request_id)
    .bind(&transcript.text)
    .bind(serde_json::to_value(&transcript.segments)?)
    .bind(&transcript.raw_json)
    .bind(job.attempts)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn save_voice_result(
    pool: &PgPool,
    job: &VoiceJob,
    result: &CleanupResult,
    final_html: &str,
    full_text_file_id: Option<&str>,
) -> anyhow::Result<bool> {
    let query_result = sqlx::query(
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
            processing_started_at = null,
            lease_expires_at = null,
            updated_at = now()
        where id = $1 and attempts = $9 and status = 'cleaning'
          and lease_expires_at > now()
        "#,
    )
    .bind(job.id)
    .bind(&result.provider)
    .bind(&result.model)
    .bind(&result.transcript.text)
    .bind(result.transcript.mode.as_str())
    .bind(serde_json::to_value(&result.transcript.chapters)?)
    .bind(final_html)
    .bind(full_text_file_id)
    .bind(job.attempts)
    .execute(pool)
    .await?;
    Ok(query_result.rows_affected() == 1)
}
