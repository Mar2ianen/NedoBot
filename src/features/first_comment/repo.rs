use sqlx::PgPool;
use teloxide::RequestError;

use crate::features::first_comment::render::ChatLinkTarget;
use crate::llm::types::LlmTransportError;
use crate::text::{normalize_ai_markers, strip_links};

const COMMENT_JOB_LEASE_SECONDS: i64 = 10 * 60;
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
            Some(LlmTransportError::HttpStatus(status)) => Self::from_http_status(*status),
            None => Self::Transient,
        }
    }

    pub fn from_telegram_error(error: &RequestError) -> Self {
        match error {
            RequestError::RetryAfter(_) => Self::RateLimited,
            RequestError::Api(teloxide::ApiError::InvalidToken) => Self::Configuration,
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

pub async fn create_post_comment_job(
    pool: &PgPool,
    discussion_chat_id: i64,
    discussion_message_id: i32,
    source_channel_id: i64,
    source_message_id: i32,
    cleaned_post_text: &str,
    image_file_id: Option<&str>,
    image_file_unique_id: Option<&str>,
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
    .bind(discussion_chat_id)
    .bind(discussion_message_id)
    .bind(source_channel_id)
    .bind(source_message_id)
    .bind(cleaned_post_text)
    .bind(image_file_id)
    .bind(image_file_unique_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(id,)| id))
}

pub async fn claim_next_post_comment_job(pool: &PgPool) -> anyhow::Result<Option<PostCommentJob>> {
    let row = sqlx::query_as::<_, (i64, i64, i32, i64, i32, String, Option<String>, i32)>(
        r#"
        with candidate as (
            select id
            from post_comment_jobs
            where (status in ('pending', 'retry_wait') and next_attempt_at <= now())
               or (status = 'processing' and lease_expires_at <= now())
            order by next_attempt_at, id
            for update skip locked
            limit 1
        )
        update post_comment_jobs job
        set status = 'processing',
            attempts = job.attempts + 1,
            processing_started_at = now(),
            lease_expires_at = now() + ($1 * interval '1 second'),
            updated_at = now()
        from candidate
        where job.id = candidate.id
        returning job.id, job.discussion_chat_id, job.discussion_message_id,
                  job.source_channel_id, job.source_message_id, job.cleaned_post_text,
                  job.image_file_id, job.attempts
        "#,
    )
    .bind(COMMENT_JOB_LEASE_SECONDS)
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
        )| PostCommentJob {
            id,
            discussion_chat_id,
            discussion_message_id,
            source_channel_id,
            source_message_id,
            cleaned_post_text,
            image_file_id,
            attempts,
        },
    ))
}

pub async fn mark_post_comment_sent(
    pool: &PgPool,
    job: &PostCommentJob,
    bot_comment_message_id: i32,
) -> anyhow::Result<bool> {
    sqlx::query(
        r#"
        update post_comment_jobs
        set status = 'sent', bot_comment_message_id = $2, error_kind = null,
            lease_expires_at = null, updated_at = now()
        where id = $1 and status = 'processing' and attempts = $3
        "#,
    )
    .bind(job.id)
    .bind(bot_comment_message_id)
    .bind(job.attempts)
    .execute(pool)
    .await
    .map(|result| result.rows_affected() == 1)
    .map_err(Into::into)
}

pub async fn mark_post_comment_failed(
    pool: &PgPool,
    job: &PostCommentJob,
    error_kind: CommentErrorKind,
) -> anyhow::Result<bool> {
    let retry_delay_seconds = error_kind
        .is_retryable()
        .then(|| retry_delay_seconds(job.attempts))
        .flatten();
    let (status, delay_seconds) = match retry_delay_seconds {
        Some(delay_seconds) => ("retry_wait", delay_seconds),
        None => ("failed", 0),
    };
    sqlx::query(
        r#"
        update post_comment_jobs
        set status = $3,
            error_kind = $4,
            next_attempt_at = now() + ($5 * interval '1 second'),
            lease_expires_at = null,
            updated_at = now()
        where id = $1 and status = 'processing' and attempts = $2
        "#,
    )
    .bind(job.id)
    .bind(job.attempts)
    .bind(status)
    .bind(error_kind.as_str())
    .bind(delay_seconds)
    .execute(pool)
    .await
    .map(|result| result.rows_affected() == 1)
    .map_err(Into::into)
}

fn retry_delay_seconds(attempts: i32) -> Option<i64> {
    if attempts >= MAX_COMMENT_JOB_ATTEMPTS {
        return None;
    }

    match attempts {
        1 => Some(15),
        2 => Some(60),
        _ => None,
    }
}

pub async fn insert_llm_generation(
    pool: &PgPool,
    generation: LlmGenerationInsert<'_>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into llm_generations
            (post_comment_job_id, provider, model, prompt, image_used, response, final_html, attempts, used_search_result_id, used_chat_message_ids)
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
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
    .execute(pool)
    .await?;

    Ok(())
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
    use super::{CommentErrorKind, retry_delay_seconds};

    #[test]
    fn comment_job_retries_are_bounded() {
        assert_eq!(retry_delay_seconds(1), Some(15));
        assert_eq!(retry_delay_seconds(2), Some(60));
        assert_eq!(retry_delay_seconds(3), None);
    }

    #[test]
    fn permanent_comment_job_errors_do_not_retry() {
        assert!(!CommentErrorKind::Configuration.is_retryable());
        assert!(!CommentErrorKind::InvalidInput.is_retryable());
        assert!(CommentErrorKind::Transient.is_retryable());
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
