use sqlx::PgPool;

use crate::features::jobs::policy::{VOICE_TRANSCRIPTION_LEASE, VOICE_TRANSCRIPTION_RETRY};
use crate::features::voice::cleanup::CleanupResult;
use crate::features::voice::types::{AsrTranscript, VoiceMedia, VoiceMediaKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceJobStatus {
    Pending,
    RetryWait,
    Processing,
    Downloading,
    Transcribing,
    Cleaning,
    Delivering,
    DeliveryUnknown,
    Sent,
    Failed,
    Skipped,
}

impl VoiceJobStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::RetryWait => "retry_wait",
            Self::Processing => "processing",
            Self::Downloading => "downloading",
            Self::Transcribing => "transcribing",
            Self::Cleaning => "cleaning",
            Self::Delivering => "delivering",
            Self::DeliveryUnknown => "delivery_unknown",
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

impl TryFrom<&str> for VoiceJobStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "pending" => Ok(Self::Pending),
            "retry_wait" => Ok(Self::RetryWait),
            "processing" => Ok(Self::Processing),
            "downloading" => Ok(Self::Downloading),
            "transcribing" => Ok(Self::Transcribing),
            "cleaning" => Ok(Self::Cleaning),
            "delivering" => Ok(Self::Delivering),
            "delivery_unknown" => Ok(Self::DeliveryUnknown),
            "sent" => Ok(Self::Sent),
            "failed" => Ok(Self::Failed),
            "skipped" => Ok(Self::Skipped),
            _ => anyhow::bail!("unknown voice transcription status: {value}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceJobPhase {
    Downloading,
    Transcribing,
    Cleaning,
    Delivering,
}

impl VoiceJobPhase {
    fn as_status(self) -> VoiceJobStatus {
        match self {
            Self::Downloading => VoiceJobStatus::Downloading,
            Self::Transcribing => VoiceJobStatus::Transcribing,
            Self::Cleaning => VoiceJobStatus::Cleaning,
            Self::Delivering => VoiceJobStatus::Delivering,
        }
    }
}

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
    pub status: VoiceJobStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum VoiceTransition {
    RetryScheduled,
    TerminalFailed,
    DeliveryUnknown,
    LeaseLost,
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
    String,
    chrono::DateTime<chrono::Utc>,
);

fn row_to_job(row: VoiceJobRow) -> anyhow::Result<VoiceJob> {
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
        status,
        created_at,
    ) = row;
    Ok(VoiceJob {
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
        status: VoiceJobStatus::try_from(status.as_str())?,
        created_at,
    })
}

const JOB_COLUMNS: &str = "id, chat_id, message_id, user_id, file_id, file_unique_id, media_kind, duration_sec, file_size, mime_type, attempts, progress_message_id, status, created_at";
const JOB_COLUMNS_QUALIFIED: &str = "job.id, job.chat_id, job.message_id, job.user_id, job.file_id, job.file_unique_id, job.media_kind, job.duration_sec, job.file_size, job.mime_type, job.attempts, job.progress_message_id, job.status, job.created_at";

pub async fn enqueue_voice_job(pool: &PgPool, media: &VoiceMedia) -> anyhow::Result<Option<i64>> {
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

pub async fn claim_voice_job(pool: &PgPool, job_id: i64) -> anyhow::Result<Option<VoiceJob>> {
    let row = sqlx::query_as::<_, VoiceJobRow>(sqlx::AssertSqlSafe(format!(
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
    )))
    .bind(job_id)
    .bind(VOICE_TRANSCRIPTION_LEASE.seconds())
    .fetch_optional(pool)
    .await?;

    row.map(row_to_job).transpose()
}

pub async fn claim_next_voice_job(pool: &PgPool) -> anyhow::Result<Option<VoiceJob>> {
    let row = sqlx::query_as::<_, VoiceJobRow>(sqlx::AssertSqlSafe(format!(
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
    )))
    .bind(VOICE_TRANSCRIPTION_LEASE.seconds())
    .fetch_optional(pool)
    .await?;

    row.map(row_to_job).transpose()
}

pub async fn recover_expired_voice_deliveries(pool: &PgPool) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"
        update voice_transcription_jobs
        set status = 'delivery_unknown',
            error_kind = 'delivery_unknown',
            error = 'delivery lease expired after Telegram send started',
            processing_started_at = null,
            lease_expires_at = null,
            updated_at = now()
        where status = 'delivering'
          and lease_expires_at <= now()
        "#,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
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
    phase: VoiceJobPhase,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"
        update voice_transcription_jobs
        set status = $2,
            lease_expires_at = now() + ($3 * interval '1 second'),
            updated_at = now()
        where id = $1 and attempts = $4
          and status in (
              'processing', 'downloading', 'transcribing', 'cleaning', 'delivering'
          )
          and lease_expires_at > now()
        "#,
    )
    .bind(job.id)
    .bind(phase.as_status().as_str())
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
) -> anyhow::Result<VoiceTransition> {
    let result = sqlx::query(
        r#"
        update voice_transcription_jobs
        set status = 'failed', error_kind = $2, error = $3,
            processing_started_at = null, lease_expires_at = null, updated_at = now()
        where id = $1 and attempts = $4
          and status not in ('sent', 'delivery_unknown')
        "#,
    )
    .bind(job.id)
    .bind(error_kind)
    .bind(error)
    .bind(job.attempts)
    .execute(pool)
    .await?;
    match result.rows_affected() {
        0 => Ok(VoiceTransition::LeaseLost),
        1 => Ok(VoiceTransition::TerminalFailed),
        count => anyhow::bail!("voice failure transition unexpectedly affected {count} rows"),
    }
}

pub async fn mark_voice_job_retry_or_failed(
    pool: &PgPool,
    job: &VoiceJob,
    error_kind: &str,
    error: &str,
) -> anyhow::Result<VoiceTransition> {
    let next_delay = VOICE_TRANSCRIPTION_RETRY.delay_seconds(job.attempts, None);
    if let Some(delay) = next_delay {
        let result = sqlx::query(
            r#"
            update voice_transcription_jobs
            set status = 'retry_wait', error_kind = $2, error = $3,
                next_attempt_at = now() + ($4 * interval '1 second'),
                processing_started_at = null, lease_expires_at = null, updated_at = now()
            where id = $1 and attempts = $5
              and status not in ('sent', 'delivery_unknown')
            "#,
        )
        .bind(job.id)
        .bind(error_kind)
        .bind(error)
        .bind(delay)
        .bind(job.attempts)
        .execute(pool)
        .await?;
        match result.rows_affected() {
            0 => Ok(VoiceTransition::LeaseLost),
            1 => Ok(VoiceTransition::RetryScheduled),
            count => anyhow::bail!("voice retry transition unexpectedly affected {count} rows"),
        }
    } else {
        mark_voice_job_failed(pool, job, error_kind, error).await
    }
}

pub async fn mark_voice_job_delivery_unknown(
    pool: &PgPool,
    job: &VoiceJob,
    error: &str,
) -> anyhow::Result<VoiceTransition> {
    let result = sqlx::query(
        r#"
        update voice_transcription_jobs
        set status = 'delivery_unknown',
            error_kind = 'delivery_unknown',
            error = $2,
            processing_started_at = null,
            lease_expires_at = null,
            updated_at = now()
        where id = $1 and attempts = $3 and status = 'delivering'
        "#,
    )
    .bind(job.id)
    .bind(error)
    .bind(job.attempts)
    .execute(pool)
    .await?;
    match result.rows_affected() {
        0 => Ok(VoiceTransition::LeaseLost),
        1 => Ok(VoiceTransition::DeliveryUnknown),
        count => {
            anyhow::bail!("voice delivery-unknown transition unexpectedly affected {count} rows")
        }
    }
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
    alternatives: &[AsrTranscript],
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
            asr_alternatives_json = $8,
            updated_at = now()
        where id = $1 and attempts = $9 and status in ('transcribing', 'cleaning')
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
    .bind(serde_json::to_value(alternatives)?)
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
        where id = $1 and attempts = $9 and status = 'delivering'
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

#[cfg(test)]
mod tests {
    use super::{VoiceJobPhase, VoiceJobStatus};

    #[test]
    fn persisted_voice_statuses_round_trip_and_unknown_values_are_rejected() {
        for status in [
            VoiceJobStatus::Pending,
            VoiceJobStatus::RetryWait,
            VoiceJobStatus::Processing,
            VoiceJobStatus::Downloading,
            VoiceJobStatus::Transcribing,
            VoiceJobStatus::Cleaning,
            VoiceJobStatus::Delivering,
            VoiceJobStatus::DeliveryUnknown,
            VoiceJobStatus::Sent,
            VoiceJobStatus::Failed,
            VoiceJobStatus::Skipped,
        ] {
            assert_eq!(
                VoiceJobStatus::try_from(status.as_str()).expect("stored status round trips"),
                status
            );
        }
        assert!(VoiceJobStatus::try_from("unknown_status").is_err());
    }

    #[test]
    fn phase_projection_only_exposes_active_pipeline_phases() {
        assert_eq!(
            VoiceJobPhase::Downloading.as_status(),
            VoiceJobStatus::Downloading
        );
        assert_eq!(
            VoiceJobPhase::Transcribing.as_status(),
            VoiceJobStatus::Transcribing
        );
        assert_eq!(
            VoiceJobPhase::Cleaning.as_status(),
            VoiceJobStatus::Cleaning
        );
        assert_eq!(
            VoiceJobPhase::Delivering.as_status(),
            VoiceJobStatus::Delivering
        );
    }
}
