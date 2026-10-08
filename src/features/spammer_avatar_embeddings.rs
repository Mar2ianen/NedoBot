use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use sqlx::{PgPool, Postgres, Row, Transaction};
use teloxide::{
    net::Download,
    prelude::{Bot, Requester},
    types::FileId,
};
use tokio::io::AsyncWrite;

use crate::{
    config::Config,
    features::{
        jobs::{
            claim::CasResult,
            policy::{
                SPAMMER_AVATAR_EMBEDDING_LEASE, SPAMMER_AVATAR_EMBEDDING_POLL,
                SPAMMER_AVATAR_EMBEDDING_RETRY,
            },
        },
        memory::embedding::{embed_profile_image, pgvector_literal},
    },
};

const MAX_AVATAR_BYTES: usize = 10 * 1024 * 1024;
const AVATAR_REQUEST_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug)]
struct AvatarEmbeddingJob {
    chat_id: i64,
    user_id: i64,
    avatar_file_unique_id: String,
    avatar_file_id: String,
    attempts: i32,
}

/// Adds the current photo to the positive dataset only after the moderation
/// transaction has set `is_spammer` to true.
pub async fn enqueue_confirmed_avatar_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    chat_id: i64,
    user_id: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into spammer_avatar_embeddings
            (chat_id, telegram_user_id, avatar_file_unique_id, avatar_file_id)
        select cu.chat_id,
               cu.telegram_user_id,
               coalesce(p.profile_photo_file_unique_id, audit.avatar_file_unique_id,
                        p.profile_photo_file_id, audit.avatar_file_id),
               coalesce(p.profile_photo_file_id, audit.avatar_file_id)
        from telegram_chat_users cu
        left join telegram_user_profiles p on p.telegram_user_id = cu.telegram_user_id
        left join lateral (
            select avatar_file_id, avatar_file_unique_id
            from new_user_audit_jobs
            where chat_id = cu.chat_id and telegram_user_id = cu.telegram_user_id
            order by created_at desc
            limit 1
        ) audit on true
        where cu.chat_id = $1
          and cu.telegram_user_id = $2
          and cu.is_spammer
          and coalesce(p.profile_photo_file_id, audit.avatar_file_id) is not null
        on conflict (chat_id, telegram_user_id, avatar_file_unique_id) do nothing
        "#,
    )
    .bind(chat_id)
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Captures any profile photo learned while an already-confirmed spammer is
/// refreshed. The caller uses the same feature gate as the worker startup.
pub async fn enqueue_refreshed_spammer_avatars(
    pool: &PgPool,
    enabled: bool,
    user_id: i64,
) -> anyhow::Result<u64> {
    if !enabled {
        return Ok(0);
    }
    let result = sqlx::query(
        r#"
        insert into spammer_avatar_embeddings
            (chat_id, telegram_user_id, avatar_file_unique_id, avatar_file_id)
        select cu.chat_id,
               cu.telegram_user_id,
               coalesce(p.profile_photo_file_unique_id, p.profile_photo_file_id),
               p.profile_photo_file_id
        from telegram_chat_users cu
        join telegram_user_profiles p on p.telegram_user_id = cu.telegram_user_id
        where cu.telegram_user_id = $1
          and cu.is_spammer
          and p.profile_photo_file_id is not null
        on conflict (chat_id, telegram_user_id, avatar_file_unique_id) do nothing
        "#,
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Starts a bounded one-at-a-time backfill for photos already cached on
/// confirmed spammers. No rows are created while the feature is disabled.
pub async fn enqueue_existing_spammer_avatars(pool: &PgPool) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"
        insert into spammer_avatar_embeddings
            (chat_id, telegram_user_id, avatar_file_unique_id, avatar_file_id)
        select cu.chat_id,
               cu.telegram_user_id,
               coalesce(p.profile_photo_file_unique_id, audit.avatar_file_unique_id,
                        p.profile_photo_file_id, audit.avatar_file_id),
               coalesce(p.profile_photo_file_id, audit.avatar_file_id)
        from telegram_chat_users cu
        left join telegram_user_profiles p on p.telegram_user_id = cu.telegram_user_id
        left join lateral (
            select avatar_file_id, avatar_file_unique_id
            from new_user_audit_jobs
            where chat_id = cu.chat_id and telegram_user_id = cu.telegram_user_id
            order by created_at desc
            limit 1
        ) audit on true
        where cu.is_spammer
          and coalesce(p.profile_photo_file_id, audit.avatar_file_id) is not null
        on conflict (chat_id, telegram_user_id, avatar_file_unique_id) do nothing
        "#,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn process_next_spammer_avatar_embedding(
    bot: &Bot,
    pool: &PgPool,
    config: &Config,
) -> anyhow::Result<bool> {
    let Some(job) = claim_next_job(pool).await? else {
        return Ok(false);
    };

    let result = tokio::time::timeout(AVATAR_REQUEST_TIMEOUT, async {
        let image_base64 = download_avatar_base64(bot, &job.avatar_file_id).await?;
        let embedding = embed_profile_image(config, &image_base64).await?;
        pgvector_literal(&embedding)
    })
    .await;

    match result {
        Ok(Ok(embedding)) => {
            let finalized = mark_ready(pool, config, &job, &embedding).await?;
            if finalized == CasResult::LeaseLost {
                tracing::debug!(
                    chat_id = job.chat_id,
                    user_id = job.user_id,
                    "spammer avatar embedding finalization lost lease"
                );
            }
        }
        Ok(Err(error)) => {
            let error_kind = if error.to_string().contains("exceeds configured limit") {
                "avatar_too_large"
            } else if error.to_string().contains("embedding") {
                "embedding_failed"
            } else {
                "avatar_download_failed"
            };
            mark_failed(pool, &job, error_kind).await?;
            tracing::warn!(
                chat_id = job.chat_id,
                user_id = job.user_id,
                error_kind,
                "spammer avatar embedding job failed"
            );
        }
        Err(_) => {
            mark_failed(pool, &job, "request_timeout").await?;
            tracing::warn!(
                chat_id = job.chat_id,
                user_id = job.user_id,
                "spammer avatar embedding job timed out"
            );
        }
    }
    Ok(true)
}

pub fn worker_idle_seconds() -> u64 {
    SPAMMER_AVATAR_EMBEDDING_POLL.idle_seconds()
}

pub fn worker_error_seconds() -> u64 {
    SPAMMER_AVATAR_EMBEDDING_POLL.error_seconds()
}

async fn claim_next_job(pool: &PgPool) -> anyhow::Result<Option<AvatarEmbeddingJob>> {
    let row = sqlx::query(
        r#"
        with candidate as (
            select e.chat_id, e.telegram_user_id, e.avatar_file_unique_id
            from spammer_avatar_embeddings e
            join telegram_chat_users cu
              on cu.chat_id = e.chat_id
             and cu.telegram_user_id = e.telegram_user_id
             and cu.is_spammer
            where (e.status in ('pending', 'retry_wait') and e.next_attempt_at <= now())
               or (e.status = 'processing' and e.lease_expires_at <= now())
            order by e.next_attempt_at, e.created_at
            for update of e skip locked
            limit 1
        )
        update spammer_avatar_embeddings e
        set status = 'processing',
            attempts = e.attempts + 1,
            lease_reclaim_count = e.lease_reclaim_count
                + case when e.status = 'processing' then 1 else 0 end,
            processing_started_at = now(),
            lease_expires_at = now() + ($1 * interval '1 second'),
            updated_at = now()
        from candidate
        where e.chat_id = candidate.chat_id
          and e.telegram_user_id = candidate.telegram_user_id
          and e.avatar_file_unique_id = candidate.avatar_file_unique_id
        returning e.chat_id, e.telegram_user_id, e.avatar_file_unique_id,
                  e.avatar_file_id, e.attempts
        "#,
    )
    .bind(SPAMMER_AVATAR_EMBEDDING_LEASE.seconds())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| AvatarEmbeddingJob {
        chat_id: row.get("chat_id"),
        user_id: row.get("telegram_user_id"),
        avatar_file_unique_id: row.get("avatar_file_unique_id"),
        avatar_file_id: row.get("avatar_file_id"),
        attempts: row.get("attempts"),
    }))
}

async fn mark_ready(
    pool: &PgPool,
    config: &Config,
    job: &AvatarEmbeddingJob,
    embedding: &str,
) -> anyhow::Result<CasResult> {
    let update = sqlx::query(
        r#"
        update spammer_avatar_embeddings e
        set embedding = $5::vector,
            embedding_model = $6,
            dataset_avatar_file_id = avatar_file_id,
            avatar_file_id = null,
            status = 'ready',
            processing_started_at = null,
            lease_expires_at = null,
            error_kind = null,
            updated_at = now()
        where e.chat_id = $1 and e.telegram_user_id = $2
          and e.avatar_file_unique_id = $3 and e.attempts = $4
          and e.status = 'processing'
          and exists (
              select 1 from telegram_chat_users cu
              where cu.chat_id = e.chat_id
                and cu.telegram_user_id = e.telegram_user_id
                and cu.is_spammer
          )
        "#,
    )
    .bind(job.chat_id)
    .bind(job.user_id)
    .bind(&job.avatar_file_unique_id)
    .bind(job.attempts)
    .bind(embedding)
    .bind(&config.rag_embedding_model)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(update.rows_affected())
}

async fn mark_failed(
    pool: &PgPool,
    job: &AvatarEmbeddingJob,
    error_kind: &str,
) -> anyhow::Result<CasResult> {
    let (status, delay) = if error_kind == "avatar_too_large" {
        ("failed", 0)
    } else {
        SPAMMER_AVATAR_EMBEDDING_RETRY
            .delay_seconds(job.attempts, None)
            .map(|delay| ("retry_wait", delay))
            .unwrap_or(("failed", 0))
    };
    let update = sqlx::query(
        r#"
        update spammer_avatar_embeddings
        set status = $4,
            error_kind = $5,
            next_attempt_at = now() + ($6 * interval '1 second'),
            processing_started_at = null,
            lease_expires_at = null,
            updated_at = now()
        where chat_id = $1 and telegram_user_id = $2
          and avatar_file_unique_id = $3 and attempts = $7 and status = 'processing'
        "#,
    )
    .bind(job.chat_id)
    .bind(job.user_id)
    .bind(&job.avatar_file_unique_id)
    .bind(status)
    .bind(error_kind)
    .bind(delay)
    .bind(job.attempts)
    .execute(pool)
    .await?;
    CasResult::from_rows_affected(update.rows_affected())
}

async fn download_avatar_base64(bot: &Bot, file_id: &str) -> anyhow::Result<String> {
    let file = bot.get_file(FileId(file_id.to_string())).await?;
    if usize::try_from(file.size).unwrap_or(usize::MAX) > MAX_AVATAR_BYTES {
        anyhow::bail!("profile avatar exceeds configured limit");
    }
    let mut writer = BoundedAvatarWriter::default();
    bot.download_file(&file.path, &mut writer).await?;
    let bytes = writer.bytes;
    if bytes.is_empty() {
        anyhow::bail!("profile avatar download was empty");
    }
    Ok(BASE64.encode(bytes))
}

#[derive(Default)]
struct BoundedAvatarWriter {
    bytes: Vec<u8>,
}

impl AsyncWrite for BoundedAvatarWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if buffer.len() > MAX_AVATAR_BYTES.saturating_sub(self.bytes.len()) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "profile avatar exceeds configured limit",
            )));
        }
        self.bytes.extend_from_slice(buffer);
        Poll::Ready(Ok(buffer.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
