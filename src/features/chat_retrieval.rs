use sqlx::{PgPool, Row};

use crate::config::Config;
use crate::features::memory::embedding::{embed_text_batch, pgvector_literal};

const LEASE_SECONDS: i64 = 10 * 60;
const MAX_RETRY_ATTEMPTS: i32 = 5;

#[derive(Debug)]
struct EmbeddingJob {
    chat_id: i64,
    message_id: i32,
    text: String,
    attempts: i32,
}

pub async fn enqueue_message_embedding(
    pool: &PgPool,
    chat_id: i64,
    message_id: i32,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into telegram_message_embeddings (chat_id, message_id, status)
        select m.chat_id, m.message_id, 'pending'
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1
          and m.message_id = $2
          and nullif(trim(m.text), '') is not null
          and m.user_id is not null
          and coalesce(p.is_bot, false) = false
          and m.is_automatic_forward = false
          and m.deleted_by_bot_at is null
          and m.spam_marked_at is null
        on conflict (chat_id, message_id) do update set
            embedding = null,
            embedding_model = null,
            status = 'pending',
            attempts = 0,
            next_attempt_at = now(),
            processing_started_at = null,
            lease_expires_at = null,
            error_kind = null,
            updated_at = now()
        where telegram_message_embeddings.status <> 'processing'
        "#,
    )
    .bind(chat_id)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn enqueue_backfill_batch(
    pool: &PgPool,
    chat_id: i64,
    limit: i64,
) -> anyhow::Result<usize> {
    let result = sqlx::query(
        r#"
        insert into telegram_message_embeddings (chat_id, message_id, status)
        select m.chat_id, m.message_id, 'pending'
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_message_embeddings e
          on e.chat_id = m.chat_id and e.message_id = m.message_id
        where m.chat_id = $1
          and e.chat_id is null
          and nullif(trim(m.text), '') is not null
          and m.user_id is not null
          and coalesce(p.is_bot, false) = false
          and m.is_automatic_forward = false
          and m.deleted_by_bot_at is null
          and m.spam_marked_at is null
        order by m.created_at asc, m.message_id asc
        limit $2
        on conflict do nothing
        "#,
    )
    .bind(chat_id)
    .bind(limit.clamp(1, 1_000))
    .execute(pool)
    .await?;
    Ok(result.rows_affected() as usize)
}

pub async fn process_next_embedding_batch(pool: &PgPool, config: &Config) -> anyhow::Result<bool> {
    let jobs = claim_embedding_jobs(pool, config.chat_retrieval_embedding_batch_size).await?;
    if jobs.is_empty() {
        return Ok(false);
    }

    let texts = jobs.iter().map(|job| job.text.as_str()).collect::<Vec<_>>();
    match embed_text_batch(config, &texts).await {
        Ok(embeddings) => {
            for (job, embedding) in jobs.iter().zip(embeddings) {
                mark_embedding_ready(pool, job, &embedding, &config.rag_embedding_model).await?;
            }
        }
        Err(err) => {
            let error_kind = if err.to_string().contains("429") {
                "http_429"
            } else {
                "embedding_failed"
            };
            for job in &jobs {
                mark_embedding_failed(pool, job, error_kind).await?;
            }
            tracing::warn!(%err, jobs = jobs.len(), "chat retrieval embedding batch failed");
        }
    }
    Ok(true)
}

async fn claim_embedding_jobs(
    pool: &PgPool,
    batch_size: usize,
) -> anyhow::Result<Vec<EmbeddingJob>> {
    let rows = sqlx::query(
        r#"
        with candidate as (
            select e.chat_id, e.message_id
            from telegram_message_embeddings e
            join telegram_messages m on m.chat_id = e.chat_id and m.message_id = e.message_id
            left join telegram_user_profiles p on p.telegram_user_id = m.user_id
            where ((e.status in ('pending', 'retry_wait') and e.next_attempt_at <= now())
                   or (e.status = 'processing' and e.lease_expires_at <= now()))
              and nullif(trim(m.text), '') is not null
              and m.user_id is not null
              and coalesce(p.is_bot, false) = false
              and m.is_automatic_forward = false
              and m.deleted_by_bot_at is null
              and m.spam_marked_at is null
            order by e.next_attempt_at, e.created_at
            for update skip locked
            limit $1
        )
        update telegram_message_embeddings e
        set status = 'processing', attempts = e.attempts + 1,
            processing_started_at = now(),
            lease_expires_at = now() + ($2 * interval '1 second'), updated_at = now()
        from candidate, telegram_messages m
        where e.chat_id = candidate.chat_id and e.message_id = candidate.message_id
          and m.chat_id = e.chat_id and m.message_id = e.message_id
        returning e.chat_id, e.message_id, m.text, e.attempts
        "#,
    )
    .bind(batch_size.clamp(1, 64) as i64)
    .bind(LEASE_SECONDS)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| EmbeddingJob {
            chat_id: row.get("chat_id"),
            message_id: row.get("message_id"),
            text: row.get("text"),
            attempts: row.get("attempts"),
        })
        .collect())
}

async fn mark_embedding_ready(
    pool: &PgPool,
    job: &EmbeddingJob,
    embedding: &[f32],
    model: &str,
) -> anyhow::Result<()> {
    let embedding = pgvector_literal(embedding)?;
    sqlx::query(
        r#"
        update telegram_message_embeddings
        set embedding = $3::vector, embedding_model = $4, status = 'ready',
            error_kind = null, lease_expires_at = null, updated_at = now()
        where chat_id = $1 and message_id = $2 and status = 'processing'
        "#,
    )
    .bind(job.chat_id)
    .bind(job.message_id)
    .bind(embedding)
    .bind(model)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_embedding_failed(
    pool: &PgPool,
    job: &EmbeddingJob,
    error_kind: &str,
) -> anyhow::Result<()> {
    let (status, delay) = retry_after(job.attempts)
        .map(|delay| ("retry_wait", delay))
        .unwrap_or(("failed", 0));
    sqlx::query(
        r#"
        update telegram_message_embeddings
        set status = $3, error_kind = $4,
            next_attempt_at = now() + ($5 * interval '1 second'),
            lease_expires_at = null, updated_at = now()
        where chat_id = $1 and message_id = $2 and status = 'processing'
        "#,
    )
    .bind(job.chat_id)
    .bind(job.message_id)
    .bind(status)
    .bind(error_kind)
    .bind(delay)
    .execute(pool)
    .await?;
    Ok(())
}

fn retry_after(attempts: i32) -> Option<i64> {
    (attempts < MAX_RETRY_ATTEMPTS).then(|| 15 * 2_i64.pow(attempts.saturating_sub(1) as u32))
}

#[cfg(test)]
mod tests {
    use super::retry_after;

    #[test]
    fn retries_are_bounded_and_increase_geometrically() {
        assert_eq!(retry_after(1), Some(15));
        assert_eq!(retry_after(2), Some(30));
        assert_eq!(retry_after(3), Some(60));
        assert_eq!(retry_after(4), Some(120));
        assert_eq!(retry_after(5), None);
    }
}
