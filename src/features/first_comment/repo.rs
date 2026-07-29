use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use teloxide::RequestError;

use crate::features::first_comment::render::ChatLinkTarget;
use crate::features::jobs::claim::CasResult;
use crate::llm::types::LlmTransportError;
use crate::text::{normalize_ai_markers, strip_links};

const POST_COMMENT_PROCESSING_LEASE_SECONDS: i64 = 10 * 60;
const POST_COMMENT_DELIVERY_LEASE_SECONDS: i64 = 60;
const MAX_COMMENT_JOB_ATTEMPTS: i32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentErrorKind {
    Configuration,
    InvalidInput,
    ImageUnavailable,
    RateLimited,
    Transient,
}

impl CommentErrorKind {
    pub fn from_http_status(status: u16) -> Self {
        match status {
            400 | 404 => Self::InvalidInput,
            401 | 403 => Self::Configuration,
            429 => Self::RateLimited,
            _ => Self::Transient,
        }
    }

    pub fn from_llm_error(error: &anyhow::Error) -> Self {
        match error.downcast_ref::<LlmTransportError>() {
            Some(LlmTransportError::Configuration) => Self::Configuration,
            Some(LlmTransportError::EmptyResponse) => Self::Transient,
            Some(LlmTransportError::HttpStatus(status)) => Self::from_http_status(*status),
            None => Self::Transient,
        }
    }

    fn from_telegram_api_error(error: &teloxide::ApiError) -> Self {
        match error {
            teloxide::ApiError::InvalidToken => Self::Configuration,
            _ => Self::Transient,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::InvalidInput => "invalid_input",
            Self::ImageUnavailable => "image_unavailable",
            Self::RateLimited => "rate_limited",
            Self::Transient => "transient",
        }
    }

    fn is_retryable(self) -> bool {
        !matches!(self, Self::Configuration | Self::InvalidInput)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendFailure {
    Confirmed {
        error_kind: CommentErrorKind,
        retry_after_seconds: Option<i64>,
    },
    DeliveryUnknown,
}

pub fn classify_send_error(error: &RequestError) -> SendFailure {
    match error {
        RequestError::RetryAfter(seconds) => SendFailure::Confirmed {
            error_kind: CommentErrorKind::RateLimited,
            retry_after_seconds: Some(i64::from(seconds.seconds())),
        },
        RequestError::Api(error) => SendFailure::Confirmed {
            error_kind: CommentErrorKind::from_telegram_api_error(error),
            retry_after_seconds: None,
        },
        RequestError::MigrateToChatId(_) => SendFailure::Confirmed {
            error_kind: CommentErrorKind::Transient,
            retry_after_seconds: None,
        },
        RequestError::Network(_) | RequestError::InvalidJson { .. } | RequestError::Io(_) => {
            SendFailure::DeliveryUnknown
        }
    }
}

#[derive(Debug, Clone)]
pub struct PostCommentJob {
    pub id: i64,
    pub discussion_chat_id: i64,
    pub discussion_message_id: i32,
    pub source_channel_id: i64,
    pub source_message_id: i32,
    pub cleaned_post_text: String,
    pub image_file_id: Option<String>,
    pub attempts: i32,
    pub operator_retry_only: bool,
}

// Used by the dedicated reconciliation binary; main.rs compiles a separate module tree.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PostCommentJobReconciliationView {
    pub id: i64,
    pub discussion_message_id: i32,
    pub source_message_id: i32,
    pub status: String,
    pub error_kind: Option<String>,
    pub attempts: i32,
    pub operator_retry_only: bool,
    pub bot_comment_message_id: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// Used by the dedicated reconciliation binary; main.rs compiles a separate module tree.
#[allow(dead_code)]
pub struct OperatorAuditParams<'a> {
    pub actor: &'a str,
    pub reason: &'a str,
}

pub struct LlmGenerationInsert<'a> {
    pub job_id: i64,
    pub provider: &'a str,
    pub model: &'a str,
    pub prompt: &'a str,
    pub image_used: bool,
    pub response: &'a str,
    pub final_html: &'a str,
    pub attempts: &'a serde_json::Value,
    pub used_search_result_id: Option<i32>,
    pub used_chat_message_ids: &'a [i32],
}

pub struct FinalizePostCommentSent<'a> {
    pub bot_comment_message_id: i32,
    pub generation: LlmGenerationInsert<'a>,
    pub history_used_search_result: Option<&'a serde_json::Value>,
    pub source_channel_id: i64,
    pub source_message_id: i32,
    pub cleaned_post_text: &'a str,
    pub bot_comment: &'a str,
}

pub struct CreatePostCommentJobParams<'a> {
    pub discussion_chat_id: i64,
    pub discussion_message_id: i32,
    pub source_channel_id: i64,
    pub source_message_id: i32,
    pub cleaned_post_text: &'a str,
    pub image_file_id: Option<&'a str>,
    pub image_file_unique_id: Option<&'a str>,
}

pub async fn create_post_comment_job(
    pool: &PgPool,
    params: CreatePostCommentJobParams<'_>,
) -> anyhow::Result<Option<i64>> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        insert into post_comment_jobs
            (discussion_chat_id, discussion_message_id, source_channel_id, source_message_id,
             cleaned_post_text, image_file_id, image_file_unique_id)
        values ($1, $2, $3, $4, $5, $6, $7)
        on conflict do nothing
        returning id
        "#,
    )
    .bind(params.discussion_chat_id)
    .bind(params.discussion_message_id)
    .bind(params.source_channel_id)
    .bind(params.source_message_id)
    .bind(params.cleaned_post_text)
    .bind(params.image_file_id)
    .bind(params.image_file_unique_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(id,)| id))
}

pub async fn claim_next_post_comment_job(pool: &PgPool) -> anyhow::Result<Option<PostCommentJob>> {
    let row = sqlx::query_as::<_, (i64, i64, i32, i64, i32, String, Option<String>, i32, bool)>(
        r#"
        with expired_sends as (
            update post_comment_jobs
            set status = 'delivery_unknown',
                error_kind = 'delivery_unknown',
                lease_expires_at = null,
                updated_at = now()
            where status = 'sending' and lease_expires_at <= now()
        ), candidate as (
            select id
            from post_comment_jobs
            where (
                not operator_retry_only
                and (
                    (status in ('pending', 'retry_wait') and next_attempt_at <= now())
                    or (status = 'processing' and lease_expires_at <= now())
                )
            ) or (
                operator_retry_only
                and status = 'processing'
                and lease_expires_at <= now()
            )
            order by next_attempt_at, id
            for update skip locked
            limit 1
        )
        update post_comment_jobs job
        set status = 'processing',
            attempts = job.attempts + 1,
            processing_started_at = now(),
            sending_started_at = null,
            lease_expires_at = now() + ($1 * interval '1 second'),
            updated_at = now()
        from candidate
        where job.id = candidate.id
        returning job.id, job.discussion_chat_id, job.discussion_message_id,
                  job.source_channel_id, job.source_message_id, job.cleaned_post_text,
                  job.image_file_id, job.attempts, job.operator_retry_only
        "#,
    )
    .bind(POST_COMMENT_PROCESSING_LEASE_SECONDS)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(
            id,
            discussion_chat_id,
            discussion_message_id,
            source_channel_id,
            source_message_id,
            cleaned_post_text,
            image_file_id,
            attempts,
            operator_retry_only,
        )| PostCommentJob {
            id,
            discussion_chat_id,
            discussion_message_id,
            source_channel_id,
            source_message_id,
            cleaned_post_text,
            image_file_id,
            attempts,
            operator_retry_only,
        },
    ))
}

// Used by the dedicated reconciliation binary; main.rs compiles a separate module tree.
#[allow(dead_code)]
pub async fn list_delivery_unknown_post_comment_jobs(
    pool: &PgPool,
    limit: i64,
) -> anyhow::Result<Vec<PostCommentJobReconciliationView>> {
    let rows = sqlx::query_as::<
        _,
        (
            i64,
            i32,
            i32,
            String,
            Option<String>,
            i32,
            bool,
            Option<i32>,
            DateTime<Utc>,
            DateTime<Utc>,
        ),
    >(
        r#"
        select id, discussion_message_id, source_message_id, status, error_kind, attempts,
               operator_retry_only, bot_comment_message_id, created_at, updated_at
        from post_comment_jobs
        where status = 'delivery_unknown'
        order by updated_at, id
        limit $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| PostCommentJobReconciliationView {
            id: row.0,
            discussion_message_id: row.1,
            source_message_id: row.2,
            status: row.3,
            error_kind: row.4,
            attempts: row.5,
            operator_retry_only: row.6,
            bot_comment_message_id: row.7,
            created_at: row.8,
            updated_at: row.9,
        })
        .collect())
}

// Used by the dedicated reconciliation binary; main.rs compiles a separate module tree.
#[allow(dead_code)]
pub async fn inspect_post_comment_job(
    pool: &PgPool,
    job_id: i64,
) -> anyhow::Result<Option<PostCommentJobReconciliationView>> {
    let row = sqlx::query_as::<
        _,
        (
            i64,
            i32,
            i32,
            String,
            Option<String>,
            i32,
            bool,
            Option<i32>,
            DateTime<Utc>,
            DateTime<Utc>,
        ),
    >(
        r#"
        select id, discussion_message_id, source_message_id, status, error_kind, attempts,
               operator_retry_only, bot_comment_message_id, created_at, updated_at
        from post_comment_jobs where id = $1
        "#,
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| PostCommentJobReconciliationView {
        id: row.0,
        discussion_message_id: row.1,
        source_message_id: row.2,
        status: row.3,
        error_kind: row.4,
        attempts: row.5,
        operator_retry_only: row.6,
        bot_comment_message_id: row.7,
        created_at: row.8,
        updated_at: row.9,
    }))
}

// Used by the dedicated reconciliation binary; main.rs compiles a separate module tree.
#[allow(dead_code)]
pub async fn mark_delivery_unknown_post_comment_delivered(
    pool: &PgPool,
    job_id: i64,
    bot_comment_message_id: i32,
    audit: OperatorAuditParams<'_>,
) -> anyhow::Result<CasResult> {
    operator_transition(
        pool,
        job_id,
        "mark_delivered",
        "sent",
        audit,
        Some(bot_comment_message_id),
    )
    .await
}

// Used by the dedicated reconciliation binary; main.rs compiles a separate module tree.
#[allow(dead_code)]
pub async fn mark_delivery_unknown_post_comment_failed(
    pool: &PgPool,
    job_id: i64,
    audit: OperatorAuditParams<'_>,
) -> anyhow::Result<CasResult> {
    operator_transition(pool, job_id, "mark_failed", "failed", audit, None).await
}

// Reached through the reconciliation-only public transitions above.
#[allow(dead_code)]
async fn operator_transition(
    pool: &PgPool,
    job_id: i64,
    action: &str,
    resulting_status: &str,
    audit: OperatorAuditParams<'_>,
    bot_comment_message_id: Option<i32>,
) -> anyhow::Result<CasResult> {
    validate_operator_audit(&audit)?;
    let mut transaction = pool.begin().await?;
    let result = sqlx::query(
        r#"
        update post_comment_jobs
        set status = $2,
            bot_comment_message_id = coalesce($3, bot_comment_message_id),
            sent_at = case when $2 = 'sent' then now() else sent_at end,
            error_kind = case when $2 = 'failed' then 'operator_marked_failed' else null end,
            processing_started_at = null,
            sending_started_at = null,
            lease_expires_at = null,
            operator_retry_only = false,
            updated_at = now()
        where id = $1 and status = 'delivery_unknown'
        "#,
    )
    .bind(job_id)
    .bind(resulting_status)
    .bind(bot_comment_message_id)
    .execute(&mut *transaction)
    .await?;
    if result.rows_affected() == 0 {
        transaction.rollback().await?;
        return Ok(CasResult::LeaseLost);
    }
    insert_operator_audit(
        &mut transaction,
        job_id,
        action,
        "delivery_unknown",
        resulting_status,
        &audit,
    )
    .await?;
    transaction.commit().await?;
    Ok(CasResult::Applied)
}

// Used by the dedicated reconciliation binary; main.rs compiles a separate module tree.
#[allow(dead_code)]
pub async fn claim_delivery_unknown_post_comment_for_operator_retry(
    pool: &PgPool,
    job_id: i64,
    audit: OperatorAuditParams<'_>,
) -> anyhow::Result<Option<PostCommentJob>> {
    validate_operator_audit(&audit)?;
    let mut transaction = pool.begin().await?;
    let row = sqlx::query_as::<_, (i64, i64, i32, i64, i32, String, Option<String>, i32, bool)>(
        r#"
        update post_comment_jobs
        set status = 'processing', attempts = attempts + 1, processing_started_at = now(),
            sending_started_at = null,
            lease_expires_at = now() + ($2 * interval '1 second'),
            operator_retry_only = true, updated_at = now()
        where id = $1 and status = 'delivery_unknown'
        returning id, discussion_chat_id, discussion_message_id, source_channel_id, source_message_id,
                  cleaned_post_text, image_file_id, attempts, operator_retry_only
        "#,
    )
    .bind(job_id)
    .bind(POST_COMMENT_PROCESSING_LEASE_SECONDS)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(row) = row else {
        transaction.rollback().await?;
        return Ok(None);
    };
    insert_operator_audit(
        &mut transaction,
        job_id,
        "retry",
        "delivery_unknown",
        "processing",
        &audit,
    )
    .await?;
    transaction.commit().await?;
    Ok(Some(PostCommentJob {
        id: row.0,
        discussion_chat_id: row.1,
        discussion_message_id: row.2,
        source_channel_id: row.3,
        source_message_id: row.4,
        cleaned_post_text: row.5,
        image_file_id: row.6,
        attempts: row.7,
        operator_retry_only: row.8,
    }))
}

pub async fn mark_operator_retry_post_comment_terminal_failed(
    pool: &PgPool,
    job: &PostCommentJob,
    error_kind: CommentErrorKind,
) -> anyhow::Result<CasResult> {
    let result = sqlx::query(
        r#"
        update post_comment_jobs
        set status = 'failed', error_kind = $3, processing_started_at = null,
            sending_started_at = null, lease_expires_at = null, updated_at = now()
        where id = $1 and status in ('processing', 'sending') and attempts = $2
          and operator_retry_only
        "#,
    )
    .bind(job.id)
    .bind(job.attempts)
    .bind(error_kind.as_str())
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(result.rows_affected())
}

// Reached through the reconciliation-only public transitions above.
#[allow(dead_code)]
async fn insert_operator_audit(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: i64,
    action: &str,
    previous_status: &str,
    resulting_status: &str,
    audit: &OperatorAuditParams<'_>,
) -> anyhow::Result<()> {
    sqlx::query(
        "insert into post_comment_job_operator_audit (post_comment_job_id, action, actor, reason, previous_status, resulting_status) values ($1, $2, $3, $4, $5, $6)",
    )
    .bind(job_id).bind(action).bind(audit.actor).bind(audit.reason)
    .bind(previous_status).bind(resulting_status)
    .execute(&mut **transaction).await?;
    Ok(())
}

// Reached through the reconciliation-only public transitions above.
#[allow(dead_code)]
fn validate_operator_audit(audit: &OperatorAuditParams<'_>) -> anyhow::Result<()> {
    if audit.actor.is_empty() || audit.actor.chars().count() > 128 {
        anyhow::bail!("--actor must contain 1..=128 characters");
    }
    if audit.reason.is_empty() || audit.reason.chars().count() > 1000 {
        anyhow::bail!("--reason must contain 1..=1000 characters");
    }
    Ok(())
}

pub async fn begin_post_comment_delivery(
    pool: &PgPool,
    job: &PostCommentJob,
) -> anyhow::Result<CasResult> {
    let result = sqlx::query(
        r#"
        update post_comment_jobs
        set status = 'sending',
            sending_started_at = now(),
            lease_expires_at = now() + ($3 * interval '1 second'),
            updated_at = now()
        where id = $1 and status = 'processing' and attempts = $2
          and lease_expires_at > now()
        "#,
    )
    .bind(job.id)
    .bind(job.attempts)
    .bind(POST_COMMENT_DELIVERY_LEASE_SECONDS)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(result.rows_affected())
}

pub async fn mark_post_comment_pre_send_failed(
    pool: &PgPool,
    job: &PostCommentJob,
    error_kind: CommentErrorKind,
) -> anyhow::Result<CasResult> {
    mark_post_comment_failed(pool, job, "processing", error_kind, None).await
}

pub async fn mark_post_comment_send_rejected(
    pool: &PgPool,
    job: &PostCommentJob,
    error_kind: CommentErrorKind,
    retry_after_seconds: Option<i64>,
) -> anyhow::Result<CasResult> {
    mark_post_comment_failed(pool, job, "sending", error_kind, retry_after_seconds).await
}

async fn mark_post_comment_failed(
    pool: &PgPool,
    job: &PostCommentJob,
    expected_status: &str,
    error_kind: CommentErrorKind,
    retry_after_seconds: Option<i64>,
) -> anyhow::Result<CasResult> {
    let retry_delay_seconds = error_kind
        .is_retryable()
        .then(|| retry_delay_seconds(job.attempts, retry_after_seconds))
        .flatten();
    let (status, delay_seconds) = match retry_delay_seconds {
        Some(delay_seconds) => ("retry_wait", delay_seconds),
        None => ("failed", 0),
    };
    let result = sqlx::query(
        r#"
        update post_comment_jobs
        set status = $4,
            error_kind = $5,
            next_attempt_at = now() + ($6 * interval '1 second'),
            lease_expires_at = null,
            updated_at = now()
        where id = $1 and attempts = $2 and status = $3
        "#,
    )
    .bind(job.id)
    .bind(job.attempts)
    .bind(expected_status)
    .bind(status)
    .bind(error_kind.as_str())
    .bind(delay_seconds)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(result.rows_affected())
}

pub async fn mark_post_comment_delivery_unknown(
    pool: &PgPool,
    job: &PostCommentJob,
) -> anyhow::Result<CasResult> {
    let result = sqlx::query(
        r#"
        update post_comment_jobs
        set status = 'delivery_unknown',
            error_kind = 'delivery_unknown',
            lease_expires_at = null,
            updated_at = now()
        where id = $1 and status = 'sending' and attempts = $2
        "#,
    )
    .bind(job.id)
    .bind(job.attempts)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(result.rows_affected())
}

pub async fn finalize_post_comment_sent(
    pool: &PgPool,
    job: &PostCommentJob,
    completed: FinalizePostCommentSent<'_>,
) -> anyhow::Result<CasResult> {
    // History persistence must share this transaction with the delivery CAS.
    // Keep the established pool-based helper linked for non-transactional callers.
    let _legacy_enqueue = crate::features::memory::service::enqueue_post_history;
    let mut transaction = pool.begin().await?;
    let result =
        finalize_post_comment_sent_in_transaction(&mut transaction, job, &completed).await?;
    if result == CasResult::LeaseLost {
        transaction.rollback().await?;
        return Ok(result);
    }

    insert_llm_generation_in_transaction(&mut transaction, &completed.generation).await?;
    enqueue_post_history_in_transaction(&mut transaction, &completed).await?;
    transaction.commit().await?;
    Ok(CasResult::Applied)
}

async fn finalize_post_comment_sent_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    job: &PostCommentJob,
    completed: &FinalizePostCommentSent<'_>,
) -> anyhow::Result<CasResult> {
    let result = sqlx::query(
        r#"
        update post_comment_jobs
        set status = 'sent', bot_comment_message_id = $2, sent_at = now(),
            error_kind = null, lease_expires_at = null, updated_at = now()
        where id = $1 and status = 'sending' and attempts = $3
        "#,
    )
    .bind(job.id)
    .bind(completed.bot_comment_message_id)
    .bind(job.attempts)
    .execute(&mut **transaction)
    .await?;
    CasResult::from_rows_affected(result.rows_affected())
}

async fn insert_llm_generation_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    generation: &LlmGenerationInsert<'_>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into llm_generations
            (post_comment_job_id, provider, model, prompt, image_used, response, final_html, attempts, used_search_result_id, used_chat_message_ids)
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        on conflict (post_comment_job_id) where post_comment_job_id is not null do nothing
        "#,
    )
    .bind(generation.job_id)
    .bind(generation.provider)
    .bind(generation.model)
    .bind(generation.prompt)
    .bind(generation.image_used)
    .bind(generation.response)
    .bind(generation.final_html)
    .bind(generation.attempts)
    .bind(generation.used_search_result_id)
    .bind(generation.used_chat_message_ids)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn enqueue_post_history_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    completed: &FinalizePostCommentSent<'_>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into post_history_entries
            (post_comment_job_id, source_channel_id, source_message_id, post_text,
             bot_comment, used_search_result)
        values ($1, $2, $3, $4, $5, $6)
        on conflict (source_channel_id, source_message_id) do nothing
        "#,
    )
    .bind(completed.generation.job_id)
    .bind(completed.source_channel_id)
    .bind(completed.source_message_id)
    .bind(completed.cleaned_post_text)
    .bind(completed.bot_comment)
    .bind(completed.history_used_search_result)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn retry_delay_seconds(attempts: i32, minimum_delay_seconds: Option<i64>) -> Option<i64> {
    if attempts >= MAX_COMMENT_JOB_ATTEMPTS {
        return None;
    }
    let delay = match attempts {
        1 => 15,
        2 => 60,
        _ => return None,
    };
    Some(delay.max(minimum_delay_seconds.unwrap_or_default()))
}

pub async fn load_recent_bot_comments(pool: &PgPool) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query_as::<_, (String,)>(
        r#"
        select coalesce(response, final_html)
        from llm_generations
        where coalesce(response, final_html) is not null
        order by created_at desc
        limit 12
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(text,)| normalize_comment_text(&text))
        .filter(|text| !text.trim().is_empty())
        .collect())
}

pub async fn load_chat_link_targets(
    pool: &PgPool,
    chat_id: i64,
    message_ids: &[i32],
) -> anyhow::Result<Vec<ChatLinkTarget>> {
    let rows = sqlx::query_as::<_, (i32, String, Option<String>)>(
        r#"
        select m.message_id,
               coalesce(nullif(trim(p.first_name), ''), nullif(trim(p.username), ''), 'Участник') as author_name,
               nullif(trim(p.username), '') as author_username
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1 and m.message_id = any($2)
          and m.deleted_by_bot_at is null and m.spam_marked_at is null
        "#,
    )
    .bind(chat_id)
    .bind(message_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(message_id, author_name, author_username)| {
            crate::features::ask::chat_search::message_url(chat_id, message_id).map(|message_url| {
                ChatLinkTarget {
                    message_id,
                    author_name,
                    author_username,
                    message_url,
                }
            })
        })
        .collect())
}

fn normalize_comment_text(text: &str) -> String {
    normalize_ai_markers(&strip_links(text))
}

#[cfg(test)]
mod tests {
    use super::{CommentErrorKind, SendFailure, classify_send_error, retry_delay_seconds};

    #[test]
    fn comment_job_retries_are_bounded() {
        assert_eq!(retry_delay_seconds(1, None), Some(15));
        assert_eq!(retry_delay_seconds(2, None), Some(60));
        assert_eq!(retry_delay_seconds(3, None), None);
    }

    #[test]
    fn retry_after_is_a_lower_bound_for_comment_retry_delay() {
        assert_eq!(retry_delay_seconds(1, Some(30)), Some(30));
        assert_eq!(retry_delay_seconds(2, Some(30)), Some(60));
    }

    #[test]
    fn permanent_comment_job_errors_do_not_retry() {
        assert!(!CommentErrorKind::Configuration.is_retryable());
        assert!(!CommentErrorKind::InvalidInput.is_retryable());
        assert!(CommentErrorKind::Transient.is_retryable());
    }

    #[test]
    fn telegram_retry_after_and_api_errors_are_confirmed() {
        let retry_after =
            teloxide::RequestError::RetryAfter(teloxide::types::Seconds::from_seconds(75));
        let api_error = teloxide::RequestError::Api(teloxide::ApiError::InvalidToken);

        assert_eq!(
            classify_send_error(&retry_after),
            SendFailure::Confirmed {
                error_kind: CommentErrorKind::RateLimited,
                retry_after_seconds: Some(75),
            }
        );
        assert_eq!(
            classify_send_error(&api_error),
            SendFailure::Confirmed {
                error_kind: CommentErrorKind::Configuration,
                retry_after_seconds: None,
            }
        );
    }

    #[test]
    fn telegram_io_error_leaves_delivery_unknown() {
        let error = teloxide::RequestError::Io(std::io::Error::other("test transport failure"));

        assert_eq!(classify_send_error(&error), SendFailure::DeliveryUnknown);
    }

    #[test]
    fn llm_http_statuses_keep_their_job_error_kind() {
        let rate_limited =
            anyhow::Error::new(crate::llm::types::LlmTransportError::http_status(429));
        let unauthorized =
            anyhow::Error::new(crate::llm::types::LlmTransportError::http_status(401));

        assert_eq!(
            CommentErrorKind::from_llm_error(&rate_limited),
            CommentErrorKind::RateLimited
        );
        assert_eq!(
            CommentErrorKind::from_llm_error(&unauthorized),
            CommentErrorKind::Configuration
        );
    }
}
