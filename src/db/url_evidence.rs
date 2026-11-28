mod identity;
mod store;

use identity::*;
pub use identity::{QrUrlEvidence, QrUrlRelation};
use store::UrlEvidenceStore;

use sqlx::PgPool;
use teloxide::types::Message;

pub const URL_EVIDENCE_PARSER_VERSION: i16 = 2;
pub const MAX_QR_URLS_PER_MESSAGE: usize = 64;
pub const MAX_URL_SHADOW_REFRESH_ATTEMPTS: i32 = 10;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BackfillSummary {
    pub candidates: usize,
    pub materialized: usize,
    pub unreadable: usize,
    pub references: usize,
    pub last_message_id: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct QrMediaCandidate {
    pub file_id: String,
    pub file_unique_id: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QrScanJob {
    pub id: i64,
    pub chat_id: i64,
    pub message_id: i32,
    pub file_id: String,
    pub file_unique_id: String,
    pub attempts: i32,
    pub lease_generation: i32,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UrlShadowRefreshJob {
    pub id: i64,
    pub instance_id: String,
    pub chat_id: i64,
    pub telegram_user_id: i64,
    pub evidence_revision: String,
    pub attempts: i32,
    pub lease_generation: i32,
}

pub fn qr_media_candidate(message: &Message) -> Option<QrMediaCandidate> {
    if let Some(photo) = message.photo().and_then(|photos| {
        photos
            .iter()
            .max_by_key(|photo| u64::from(photo.width) * u64::from(photo.height))
    }) {
        return Some(QrMediaCandidate {
            file_id: photo.file.id.to_string(),
            file_unique_id: photo.file.unique_id.to_string(),
        });
    }
    if let Some(document) = message.document()
        && document.mime_type.as_ref().is_some_and(|mime| {
            matches!(
                mime.essence_str(),
                "image/png" | "image/jpeg" | "image/webp"
            )
        })
    {
        return Some(QrMediaCandidate {
            file_id: document.file.id.to_string(),
            file_unique_id: document.file.unique_id.to_string(),
        });
    }
    if let Some(sticker) = message.sticker()
        && !sticker.is_animated()
        && !sticker.is_video()
    {
        return Some(QrMediaCandidate {
            file_id: sticker.file.id.to_string(),
            file_unique_id: sticker.file.unique_id.to_string(),
        });
    }
    None
}

pub async fn enqueue_qr_scan_job(pool: &PgPool, message: &Message) -> anyhow::Result<bool> {
    let Some(candidate) = qr_media_candidate(message) else {
        return Ok(false);
    };
    let result = sqlx::query(
        r#"insert into public.telegram_qr_scan_jobs
              (chat_id, message_id, file_id, file_unique_id)
           values ($1, $2, $3, $4)
           on conflict (chat_id, message_id, file_unique_id) do nothing"#,
    )
    .bind(message.chat.id.0)
    .bind(message.id.0)
    .bind(candidate.file_id)
    .bind(candidate.file_unique_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn supersede_qr_scan_jobs(
    pool: &PgPool,
    message: &Message,
    enabled: bool,
) -> anyhow::Result<()> {
    let current_id = enabled
        .then(|| qr_media_candidate(message).map(|candidate| candidate.file_unique_id))
        .flatten();
    sqlx::query(
        r#"update public.telegram_qr_scan_jobs
              set status = 'failed', error_kind = 'message_media_superseded',
                  lease_generation = lease_generation + 1,
                  lease_expires_at = null, updated_at = now()
            where chat_id = $1 and message_id = $2
              and status in ('pending', 'leased')
              and ($3::text is null or file_unique_id <> $3)"#,
    )
    .bind(message.chat.id.0)
    .bind(message.id.0)
    .bind(current_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn claim_next_qr_scan_job(pool: &PgPool) -> anyhow::Result<Option<QrScanJob>> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "update public.telegram_qr_scan_jobs set status = 'failed', lease_expires_at = null, error_kind = 'lease_exhausted', updated_at = now() where status = 'leased' and lease_expires_at <= now() and attempts >= 5",
    )
    .execute(&mut *tx)
    .await?;
    let job = sqlx::query_as::<_, QrScanJob>(
        r#"with candidate as (
               select id from public.telegram_qr_scan_jobs
                where attempts < 5 and retry_at <= now()
                  and (status = 'pending' or (status = 'leased' and lease_expires_at <= now()))
                order by retry_at, id
                for update skip locked
                limit 1
           )
           update public.telegram_qr_scan_jobs as job
              set status = 'leased', attempts = attempts + 1,
                  lease_generation = lease_generation + 1,
                  lease_expires_at = now() + interval '10 minutes',
                  updated_at = now()
             from candidate
            where job.id = candidate.id
        returning job.id, job.chat_id, job.message_id, job.file_id, job.file_unique_id,
                  job.attempts, job.lease_generation"#,
    )
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(job)
}

pub async fn finish_qr_scan_job(
    pool: &PgPool,
    job: &QrScanJob,
    reference_count: Option<usize>,
    error_kind: Option<&str>,
) -> anyhow::Result<()> {
    let succeeded = reference_count.is_some();
    let terminal = !succeeded && job.attempts >= 5;
    let delay_seconds = 5_i64.saturating_mul(1_i64 << job.attempts.min(10));
    let result = sqlx::query(
        r#"update public.telegram_qr_scan_jobs
              set status = case when $2 then 'succeeded' when $3 then 'failed' else 'pending' end,
                  reference_count = coalesce($4, reference_count),
                  error_kind = case when $2 then null else $5 end,
                  retry_at = case when $2 or $3 then retry_at else now() + make_interval(secs => $6) end,
                  lease_expires_at = null,
                  updated_at = now()
            where id = $1 and status = 'leased' and lease_generation = $7"#,
    )
    .bind(job.id)
    .bind(succeeded)
    .bind(terminal)
    .bind(reference_count.map(i16::try_from).transpose()?)
    .bind(error_kind)
    .bind(delay_seconds)
    .bind(job.lease_generation)
    .execute(pool)
    .await?;
    anyhow::ensure!(result.rows_affected() == 1, "QR scan lease was superseded");
    Ok(())
}

pub async fn claim_next_url_shadow_refresh_job(
    pool: &PgPool,
    instance_id: &str,
) -> anyhow::Result<Option<UrlShadowRefreshJob>> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "update public.telegram_url_shadow_refresh_jobs set status = 'failed', lease_expires_at = null, error_kind = 'lease_exhausted', updated_at = now() where instance_id = $1 and status = 'leased' and lease_expires_at <= now() and attempts >= $2",
    )
    .bind(instance_id)
    .bind(MAX_URL_SHADOW_REFRESH_ATTEMPTS)
    .execute(&mut *tx)
    .await?;
    let job = sqlx::query_as::<_, UrlShadowRefreshJob>(
        r#"with candidate as (
               select id from public.telegram_url_shadow_refresh_jobs
                where instance_id = $1 and attempts < $2 and retry_at <= now()
                  and (status = 'pending' or (status = 'leased' and lease_expires_at <= now()))
                order by retry_at, id
                for update skip locked
                limit 1
           )
           update public.telegram_url_shadow_refresh_jobs as job
              set status = 'leased', attempts = attempts + 1,
                  lease_generation = lease_generation + 1,
                  lease_expires_at = now() + interval '2 minutes',
                  updated_at = now()
             from candidate
            where job.id = candidate.id
        returning job.id, job.instance_id, job.chat_id, job.telegram_user_id,
                  job.evidence_revision, job.attempts, job.lease_generation"#,
    )
    .bind(instance_id)
    .bind(MAX_URL_SHADOW_REFRESH_ATTEMPTS)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(job)
}

pub async fn finish_url_shadow_refresh_job(
    pool: &PgPool,
    job: &UrlShadowRefreshJob,
    error_kind: Option<&str>,
) -> anyhow::Result<()> {
    let succeeded = error_kind.is_none();
    let terminal = !succeeded && job.attempts >= MAX_URL_SHADOW_REFRESH_ATTEMPTS;
    let delay_seconds = 5_i64.saturating_mul(1_i64 << job.attempts.min(10));
    let result = sqlx::query(
        r#"update public.telegram_url_shadow_refresh_jobs
              set status = case when $3 then 'completed' when $4 then 'failed' else 'pending' end,
                  error_kind = $5,
                  retry_at = case when $3 or $4 then retry_at else now() + make_interval(secs => $6) end,
                  lease_expires_at = null,
                  updated_at = now()
            where id = $1 and status = 'leased' and lease_generation = $2"#,
    )
    .bind(job.id)
    .bind(job.lease_generation)
    .bind(succeeded)
    .bind(terminal)
    .bind(error_kind)
    .bind(delay_seconds)
    .execute(pool)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        "URL shadow refresh lease was superseded"
    );
    Ok(())
}

/// Replaces this message's derived references atomically; raw targets are never copied here.
pub async fn replace_message_url_evidence(
    pool: &PgPool,
    message: &Message,
) -> anyhow::Result<usize> {
    replace_message_url_evidence_with_qr(pool, message, None).await
}

pub async fn replace_message_url_evidence_with_qr(
    pool: &PgPool,
    message: &Message,
    qr_urls: Option<&[String]>,
) -> anyhow::Result<usize> {
    let classified_qr_urls = qr_urls.map(|urls| {
        urls.iter()
            .cloned()
            .map(|target| QrUrlEvidence {
                target,
                relation: QrUrlRelation::Unresolved,
            })
            .collect::<Vec<_>>()
    });
    let mut tx = pool.begin().await?;
    let total_count =
        UrlEvidenceStore::materialize(&mut tx, message, classified_qr_urls.as_deref()).await?;
    tx.commit().await?;
    Ok(total_count)
}

/// Persists QR results only while this exact media revision still owns the lease.
/// The lease row and current Telegram message row remain locked until evidence commits.
pub async fn replace_qr_evidence_for_job(
    pool: &PgPool,
    instance_id: &str,
    job: &QrScanJob,
    qr_urls: &[QrUrlEvidence],
) -> anyhow::Result<Option<usize>> {
    let mut tx = pool.begin().await?;
    let lease_file_id: Option<String> = sqlx::query_scalar(
        "select file_unique_id from public.telegram_qr_scan_jobs where id = $1 and status = 'leased' and lease_generation = $2 for update",
    )
    .bind(job.id)
    .bind(job.lease_generation)
    .fetch_optional(&mut *tx)
    .await?;
    if lease_file_id.as_deref() != Some(job.file_unique_id.as_str()) {
        tx.rollback().await?;
        return Ok(None);
    }

    let message_row: Option<(serde_json::Value, Option<i64>)> = sqlx::query_as(
        "select raw_json, user_id from public.telegram_messages where chat_id = $1 and message_id = $2 for update",
    )
    .bind(job.chat_id)
    .bind(job.message_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((raw_json, user_id)) = message_row else {
        tx.rollback().await?;
        return Ok(None);
    };
    let message: Message = crate::telegram::entities::deserialize_stored_message(raw_json)?;
    let current_file_id = qr_media_candidate(&message).map(|candidate| candidate.file_unique_id);
    if current_file_id.as_deref() != Some(job.file_unique_id.as_str()) {
        sqlx::query(
            r#"update public.telegram_qr_scan_jobs
                  set status = 'failed', error_kind = 'message_media_superseded',
                      lease_generation = lease_generation + 1,
                      lease_expires_at = null, updated_at = now()
                where id = $1 and status = 'leased' and lease_generation = $2"#,
        )
        .bind(job.id)
        .bind(job.lease_generation)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(None);
    }

    let total_count = UrlEvidenceStore::materialize(&mut tx, &message, Some(qr_urls)).await?;
    if let Some(telegram_user_id) = user_id {
        let evidence_revision: String = sqlx::query_scalar(
            "select source_fingerprint::text from public.telegram_message_url_evidence_runs where chat_id = $1 and message_id = $2 and parser_version = $3",
        )
        .bind(job.chat_id)
        .bind(job.message_id)
        .bind(URL_EVIDENCE_PARSER_VERSION)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r#"insert into public.telegram_url_shadow_refresh_jobs
                  (instance_id, chat_id, telegram_user_id, evidence_revision)
               values ($1, $2, $3, $4)
               on conflict (instance_id, chat_id, telegram_user_id, evidence_revision) do nothing"#,
        )
        .bind(instance_id)
        .bind(job.chat_id)
        .bind(telegram_user_id)
        .bind(evidence_revision)
        .execute(&mut *tx)
        .await?;
    }
    let updated = sqlx::query(
        r#"update public.telegram_qr_scan_jobs
              set status = 'succeeded', reference_count = $3, error_kind = null,
                  lease_expires_at = null, updated_at = now()
            where id = $1 and status = 'leased' and lease_generation = $2"#,
    )
    .bind(job.id)
    .bind(job.lease_generation)
    .bind(i16::try_from(total_count)?)
    .execute(&mut *tx)
    .await?;
    anyhow::ensure!(updated.rows_affected() == 1, "QR scan lease was superseded");
    tx.commit().await?;
    Ok(Some(total_count))
}

pub async fn backfill_url_evidence(
    pool: &PgPool,
    chat_id: i64,
    after_message_id: i32,
    limit: i64,
    apply: bool,
) -> anyhow::Result<BackfillSummary> {
    anyhow::ensure!(
        (1..=10_000).contains(&limit),
        "limit must be between 1 and 10000"
    );
    let candidates: Vec<(i32, serde_json::Value)> = sqlx::query_as(
        r#"select message.message_id, message.raw_json
             from public.telegram_messages as message
            where message.chat_id = $1
              and message.message_id > $2
              and not exists (
                    select 1 from public.telegram_message_url_evidence_runs as run
                     where run.chat_id = message.chat_id
                       and run.message_id = message.message_id
                       and run.parser_version = $3
              )
            order by message.message_id
            limit $4"#,
    )
    .bind(chat_id)
    .bind(after_message_id)
    .bind(URL_EVIDENCE_PARSER_VERSION)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut summary = BackfillSummary {
        candidates: candidates.len(),
        ..BackfillSummary::default()
    };
    for (message_id, raw_json) in candidates {
        summary.last_message_id = Some(message_id);
        let Ok(message) = crate::telegram::entities::deserialize_stored_message(raw_json) else {
            summary.unreadable += 1;
            continue;
        };
        let count = teloxide_url::adapter::extract_from_message(&message).len();
        summary.references += count;
        if apply {
            replace_message_url_evidence(pool, &message).await?;
            summary.materialized += 1;
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use sqlx::{PgPool, query, query_as, query_scalar};
    use teloxide_url::{Found, Kind, Source};

    const TEST_CHAT_ID: i64 = -100_991_223_441;
    const TEST_MESSAGE_ID: i32 = 9_912_234;
    const TEST_LINKED_SOURCE_CHANNEL_ID: i64 = -100_991_223_442;
    const TEST_QR_INSTANCE_ID: &str = "qr-shadow-outbox-test";

    #[test]
    fn evidence_redacts_invite_payload_but_keeps_host_and_fingerprint() {
        let found = Found {
            value: "https://t.me/+SYNTHETIC_SECRET_INVITE".into(),
            kind: Kind::Url,
            source: Source::TelegramEntity,
            span: None,
            label: Some("join".into()),
            custom_emoji_fallback_id: None,
        };
        let row = encode(&found, 0, None).unwrap();
        assert_eq!(row.host.as_deref(), Some("t.me"));
        assert_eq!(row.source, "telegram_entity");
        assert!(row.privacy_sensitive);
        assert!(row.display_hint.contains("t.me"));
        assert!(!row.display_hint.contains("SYNTHETIC_SECRET"));
        assert!(!row.target_sha256.contains("SYNTHETIC_SECRET"));
    }

    #[test]
    fn same_message_keeps_target_edit_fingerprintable() {
        let a = Found::new("https://t.me/+SYNTHETIC_A", Kind::Url);
        let b = Found::new("https://t.me/+SYNTHETIC_B", Kind::Url);
        assert_ne!(
            encode(&a, 0, None).unwrap().target_sha256,
            encode(&b, 0, None).unwrap().target_sha256
        );
    }

    #[test]
    fn canonical_url_identity_preserves_path_and_query_case() {
        assert_eq!(
            canonical_url_identity("https://EXAMPLE.org/File?campaign=Normal"),
            "https://example.org/File?campaign=Normal"
        );
        assert_ne!(
            canonical_url_identity("https://example.org/File"),
            canonical_url_identity("https://example.org/file")
        );
        assert_ne!(
            canonical_url_identity("https://example.org/?campaign=Normal"),
            canonical_url_identity("https://example.org/?campaign=normal")
        );
    }

    #[test]
    fn extraction_source_and_reference_kind_are_preserved() {
        let message: Message = serde_json::from_value(json!({
            "message_id": 42, "date": 1_700_000_000,
            "chat": {"id": -1001, "type": "supergroup"},
            "from": {"id": 7, "is_bot": false, "first_name": "Tester"},
            "text": "open", "entities": [{
                "type": "text_link", "offset": 0, "length": 4,
                "url": "https://example.org/private/path"
            }]
        }))
        .unwrap();
        let found = teloxide_url::adapter::extract_from_message(&message);
        let row = encode(&found[0], 0, None).unwrap();
        assert_eq!(row.source, "telegram_entity");
        assert_eq!(row.kind, "url");
        assert_eq!(row.host.as_deref(), Some("example.org"));
        assert!(row.privacy_sensitive);
    }

    #[test]
    fn qr_media_candidates_choose_largest_photo_and_reject_other_documents() {
        let photo: Message = serde_json::from_value(json!({
            "message_id": 3, "date": 1_700_000_000,
            "chat": {"id": -1001, "type": "supergroup"},
            "from": {"id": 7, "is_bot": false, "first_name": "Test"},
            "photo": [
                {"file_id":"small", "file_unique_id":"small-unique", "width":10, "height":10},
                {"file_id":"large", "file_unique_id":"large-unique", "width":100, "height":100}
            ]
        }))
        .unwrap();
        let candidate = qr_media_candidate(&photo).unwrap();
        assert_eq!(candidate.file_id, "large");
        assert_eq!(candidate.file_unique_id, "large-unique");

        let document: Message = serde_json::from_value(json!({
            "message_id": 4, "date": 1_700_000_000,
            "chat": {"id": -1001, "type": "supergroup"},
            "from": {"id": 7, "is_bot": false, "first_name": "Test"},
            "document": {"file_id":"doc", "file_unique_id":"doc-unique", "mime_type":"application/pdf"}
        }))
        .unwrap();
        assert!(qr_media_candidate(&document).is_none());
    }

    #[tokio::test]
    #[ignore = "requires a disposable migrated PostgreSQL database"]
    async fn database_evidence_is_redacted_replaceable_and_backfillable() {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must point at a disposable test database");
        let pool = PgPool::connect(&database_url).await.unwrap();
        crate::db::migrate(&pool).await.unwrap();
        query(
            "delete from telegram_url_shadow_refresh_jobs where instance_id = $1 and chat_id = $2",
        )
        .bind(TEST_QR_INSTANCE_ID)
        .bind(TEST_CHAT_ID)
        .execute(&pool)
        .await
        .unwrap();
        let readonly_role_exists: bool =
            query_scalar("select exists(select 1 from pg_roles where rolname = 'nedobot_mcp_ro')")
                .fetch_one(&pool)
                .await
                .unwrap();
        if readonly_role_exists {
            let can_read_private_evidence: bool = query_scalar(
                "select has_table_privilege('nedobot_mcp_ro', 'public.telegram_message_url_evidence', 'select')",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(!can_read_private_evidence);
            let can_read_link_cache: bool = query_scalar(
                "select has_table_privilege('nedobot_mcp_ro', 'public.telegram_linked_chat_cache', 'select')",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(!can_read_link_cache);
        }
        query("delete from public.telegram_linked_chat_cache where source_channel_id = $1")
            .bind(TEST_LINKED_SOURCE_CHANNEL_ID)
            .execute(&pool)
            .await
            .unwrap();
        query("insert into public.telegram_linked_chat_cache (source_channel_id, linked_chat_id, observed_at) values ($1, $2, now())")
            .bind(TEST_LINKED_SOURCE_CHANNEL_ID)
            .bind(TEST_CHAT_ID)
            .execute(&pool)
            .await
            .unwrap();
        let fresh_link: Option<i64> = query_scalar(
            "select linked_chat_id from public.telegram_linked_chat_cache where source_channel_id = $1 and observed_at >= now() - interval '14 days'",
        )
        .bind(TEST_LINKED_SOURCE_CHANNEL_ID)
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(fresh_link, Some(TEST_CHAT_ID));
        query("update public.telegram_linked_chat_cache set observed_at = now() - interval '15 days' where source_channel_id = $1")
            .bind(TEST_LINKED_SOURCE_CHANNEL_ID)
            .execute(&pool)
            .await
            .unwrap();
        let stale_link: Option<i64> = query_scalar(
            "select linked_chat_id from public.telegram_linked_chat_cache where source_channel_id = $1 and observed_at >= now() - interval '14 days'",
        )
        .bind(TEST_LINKED_SOURCE_CHANNEL_ID)
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(stale_link, None);
        query("delete from public.telegram_linked_chat_cache where source_channel_id = $1")
            .bind(TEST_LINKED_SOURCE_CHANNEL_ID)
            .execute(&pool)
            .await
            .unwrap();
        query("delete from telegram_messages where chat_id = $1 and message_id between $2 and $3")
            .bind(TEST_CHAT_ID)
            .bind(TEST_MESSAGE_ID)
            .bind(TEST_MESSAGE_ID + 4)
            .execute(&pool)
            .await
            .unwrap();

        let first = hidden_link_message(TEST_MESSAGE_ID, "SYNTHETIC_SECRET_A");
        insert_message(&pool, &first).await;
        assert_eq!(
            replace_message_url_evidence(&pool, &first).await.unwrap(),
            1
        );
        let original: (String, String, String, bool) = query_as(
            "select target_sha256::text, source, display_hint, privacy_sensitive from telegram_message_url_evidence where chat_id = $1 and message_id = $2",
        )
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(original.1, "telegram_entity");
        assert_eq!(original.2, "https://t.me/…");
        assert!(original.3);
        assert!(!original.2.contains("SYNTHETIC_SECRET"));
        let marker: (String, i16, i16) = query_as(
            "select source_fingerprint::text, parser_version, target_count from telegram_message_url_evidence_runs where chat_id = $1 and message_id = $2",
        )
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(marker.1, URL_EVIDENCE_PARSER_VERSION);
        assert_eq!(marker.2, 1);
        assert!(!marker.0.contains("SYNTHETIC_SECRET"));

        let edited = hidden_link_message(TEST_MESSAGE_ID, "SYNTHETIC_SECRET_B");
        replace_message_url_evidence(&pool, &edited).await.unwrap();
        let edited_hash: String = query_scalar(
            "select target_sha256::text from telegram_message_url_evidence where chat_id = $1 and message_id = $2",
        )
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_ne!(original.0, edited_hash);

        // Replaying the same delivery cannot duplicate the current projection.
        assert_eq!(
            replace_message_url_evidence(&pool, &edited).await.unwrap(),
            1
        );
        let replayed_hash: String = query_scalar(
            "select target_sha256::text from telegram_message_url_evidence where chat_id = $1 and message_id = $2",
        )
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(replayed_hash, edited_hash);

        // The same resource coming from visible text and a hidden text_link is
        // represented by multiple provenance rows but one unique identity.
        let same_target = "https://example.invalid/shared";
        let multi_source: Message = serde_json::from_value(json!({
            "message_id": TEST_MESSAGE_ID + 4,
            "date": 1_700_000_000,
            "chat": {"id": TEST_CHAT_ID, "type": "supergroup"},
            "from": {"id": 7_777_777, "is_bot": false, "first_name": "Synthetic"},
            "text": format!("open {same_target}"),
            "entities": [{
                "type": "text_link", "offset": 0, "length": 4,
                "url": same_target
            }]
        }))
        .unwrap();
        insert_message(&pool, &multi_source).await;
        assert_eq!(
            replace_message_url_evidence(&pool, &multi_source)
                .await
                .unwrap(),
            2
        );
        let (provenance_rows, unique_targets, expected_hash_matches): (i64, i64, i64) = query_as(
            "select count(*), count(distinct target_sha256), count(*) filter (where target_sha256 = $3) from telegram_message_url_evidence where chat_id = $1 and message_id = $2",
        )
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID + 4)
        .bind(reference_url_hash(same_target))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(provenance_rows, 2);
        assert_eq!(unique_targets, 1);
        assert_eq!(expected_hash_matches, 2);

        // Removing a link on edit removes its old evidence from the projection.
        let no_link: Message = serde_json::from_value(json!({
            "message_id": TEST_MESSAGE_ID,
            "date": 1_700_000_001,
            "edit_date": 1_700_000_001,
            "chat": {"id": TEST_CHAT_ID, "type": "supergroup"},
            "from": {"id": 7_777_777, "is_bot": false, "first_name": "Synthetic"},
            "text": "edited without a link"
        }))
        .unwrap();
        assert_eq!(
            replace_message_url_evidence(&pool, &no_link).await.unwrap(),
            0
        );
        let removed_rows: i64 = query_scalar(
            "select count(*) from telegram_message_url_evidence where chat_id = $1 and message_id = $2",
        )
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(removed_rows, 0);

        let backfill = hidden_link_message(TEST_MESSAGE_ID + 1, "SYNTHETIC_BACKFILL");
        insert_message(&pool, &backfill).await;
        let preview = backfill_url_evidence(&pool, TEST_CHAT_ID, TEST_MESSAGE_ID, 10, false)
            .await
            .unwrap();
        assert_eq!(preview.candidates, 1);
        assert_eq!(preview.references, 1);
        assert_eq!(preview.materialized, 0);
        assert_eq!(preview.last_message_id, Some(TEST_MESSAGE_ID + 1));
        let before_apply: i64 = query_scalar(
            "select count(*) from telegram_message_url_evidence_runs where chat_id = $1 and message_id = $2",
        )
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID + 1)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(before_apply, 0);

        let applied = backfill_url_evidence(&pool, TEST_CHAT_ID, TEST_MESSAGE_ID, 10, true)
            .await
            .unwrap();
        assert_eq!(applied.materialized, 1);
        assert_eq!(applied.references, 1);
        let repeat = backfill_url_evidence(&pool, TEST_CHAT_ID, TEST_MESSAGE_ID, 10, true)
            .await
            .unwrap();
        assert_eq!(repeat.candidates, 0);

        let photo = photo_message(TEST_MESSAGE_ID + 2, "qr-file", "qr-file-unique");
        insert_message(&pool, &photo).await;
        assert!(enqueue_qr_scan_job(&pool, &photo).await.unwrap());
        assert!(!enqueue_qr_scan_job(&pool, &photo).await.unwrap());
        let job = claim_next_qr_scan_job(&pool).await.unwrap().unwrap();
        assert_eq!(job.chat_id, TEST_CHAT_ID);
        assert_eq!(job.message_id, TEST_MESSAGE_ID + 2);
        assert_eq!(job.file_unique_id, "qr-file-unique");
        assert_eq!(job.attempts, 1);
        let mut stale_lease = job.clone();
        stale_lease.lease_generation -= 1;
        assert!(
            finish_qr_scan_job(&pool, &stale_lease, Some(0), None)
                .await
                .is_err()
        );
        let stored = replace_qr_evidence_for_job(
            &pool,
            TEST_QR_INSTANCE_ID,
            &job,
            &[
                QrUrlEvidence {
                    target: "https://example.invalid/File".to_owned(),
                    relation: QrUrlRelation::External,
                },
                QrUrlEvidence {
                    target: "https://example.invalid/file".to_owned(),
                    relation: QrUrlRelation::External,
                },
                QrUrlEvidence {
                    target: "https://EXAMPLE.invalid/File".to_owned(),
                    relation: QrUrlRelation::External,
                },
            ],
        )
        .await
        .unwrap();
        assert_eq!(stored, Some(2));
        let evidence_revision: String = query_scalar(
            "select source_fingerprint::text from telegram_message_url_evidence_runs where chat_id = $1 and message_id = $2 and parser_version = $3",
        )
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID + 2)
        .bind(URL_EVIDENCE_PARSER_VERSION)
        .fetch_one(&pool)
        .await
        .unwrap();
        let duplicate_outbox = query(
            r#"insert into telegram_url_shadow_refresh_jobs
                  (instance_id, chat_id, telegram_user_id, evidence_revision)
               values ($1, $2, 7_777_777, $3)
               on conflict (instance_id, chat_id, telegram_user_id, evidence_revision) do nothing"#,
        )
        .bind(TEST_QR_INSTANCE_ID)
        .bind(TEST_CHAT_ID)
        .bind(&evidence_revision)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(duplicate_outbox.rows_affected(), 0);
        let refresh_count: i64 = query_scalar(
            "select count(*) from telegram_url_shadow_refresh_jobs where instance_id = $1 and chat_id = $2 and telegram_user_id = 7_777_777",
        )
        .bind(TEST_QR_INSTANCE_ID)
        .bind(TEST_CHAT_ID)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(refresh_count, 1, "same evidence revision must deduplicate");
        let scan_state: (String, i16) =
            query_as("select status, reference_count from telegram_qr_scan_jobs where id = $1")
                .bind(job.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(scan_state, ("succeeded".to_owned(), 2));
        let refresh_job = claim_next_url_shadow_refresh_job(&pool, TEST_QR_INSTANCE_ID)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refresh_job.instance_id, TEST_QR_INSTANCE_ID);
        assert_eq!(refresh_job.chat_id, TEST_CHAT_ID);
        assert_eq!(refresh_job.telegram_user_id, 7_777_777);
        assert_eq!(refresh_job.evidence_revision.len(), 64);
        assert!(!refresh_job.evidence_revision.contains("example.invalid"));
        finish_url_shadow_refresh_job(&pool, &refresh_job, Some("synthetic_failure"))
            .await
            .unwrap();
        let refresh_state: (String, Option<String>) = query_as(
            "select status, error_kind from telegram_url_shadow_refresh_jobs where id = $1",
        )
        .bind(refresh_job.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            refresh_state,
            ("pending".to_owned(), Some("synthetic_failure".to_owned()))
        );
        let qr_hashes: Vec<String> = query_scalar(
            "select target_sha256::text from telegram_message_url_evidence where chat_id = $1 and message_id = $2 and source = 'qr' order by item_index",
        )
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID + 2)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(qr_hashes.len(), 2);
        assert_ne!(qr_hashes[0], qr_hashes[1]);
        let relation: Option<String> = query_scalar(
            "select qr_relation from telegram_message_url_evidence where chat_id = $1 and message_id = $2 and source = 'qr'",
        )
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID + 2)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(relation.as_deref(), Some("external"));
        let queue_status: String = query_scalar(
            "select status from telegram_qr_scan_jobs where chat_id = $1 and message_id = $2",
        )
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID + 2)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(queue_status, "succeeded");

        let edited_photo = photo_message(TEST_MESSAGE_ID + 3, "old-qr-file", "old-unique");
        insert_message(&pool, &edited_photo).await;
        enqueue_qr_scan_job(&pool, &edited_photo).await.unwrap();
        let stale_after_edit = claim_next_qr_scan_job(&pool).await.unwrap().unwrap();
        let edited_current = photo_message(TEST_MESSAGE_ID + 3, "new-qr-file", "new-unique");
        query("update telegram_messages set raw_json = $3 where chat_id = $1 and message_id = $2")
            .bind(TEST_CHAT_ID)
            .bind(TEST_MESSAGE_ID + 3)
            .bind(serde_json::to_value(&edited_current).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        let stale_write = replace_qr_evidence_for_job(
            &pool,
            TEST_QR_INSTANCE_ID,
            &stale_after_edit,
            &[QrUrlEvidence {
                target: "https://example.invalid/stale-qr".to_owned(),
                relation: QrUrlRelation::External,
            }],
        )
        .await
        .unwrap();
        assert_eq!(stale_write, None);
        let stale_job_status: String =
            query_scalar("select status from telegram_qr_scan_jobs where id = $1")
                .bind(stale_after_edit.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stale_job_status, "failed");
        // The worker-side generation check rejects the old result even before
        // the edit handler's best-effort queue supersession runs.
        supersede_qr_scan_jobs(&pool, &edited_current, true)
            .await
            .unwrap();
        let stale_qr_rows: i64 = query_scalar(
            "select count(*) from telegram_message_url_evidence where chat_id = $1 and message_id = $2 and source = 'qr'",
        )
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID + 3)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stale_qr_rows, 0);
        let stale_refresh_count: i64 = query_scalar(
            "select count(*) from telegram_url_shadow_refresh_jobs where instance_id = $1 and chat_id = $2 and telegram_user_id = 7_777_777 and evidence_revision in (select source_fingerprint::text from telegram_message_url_evidence_runs where chat_id = $2 and message_id = $3)",
        )
        .bind(TEST_QR_INSTANCE_ID)
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID + 3)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stale_refresh_count, 0);

        query("delete from telegram_messages where chat_id = $1 and message_id between $2 and $3")
            .bind(TEST_CHAT_ID)
            .bind(TEST_MESSAGE_ID)
            .bind(TEST_MESSAGE_ID + 4)
            .execute(&pool)
            .await
            .unwrap();
        query(
            "delete from telegram_url_shadow_refresh_jobs where instance_id = $1 and chat_id = $2",
        )
        .bind(TEST_QR_INSTANCE_ID)
        .bind(TEST_CHAT_ID)
        .execute(&pool)
        .await
        .unwrap();
        let cascaded: i64 =
            query_scalar("select count(*) from telegram_message_url_evidence where chat_id = $1")
                .bind(TEST_CHAT_ID)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(cascaded, 0);
        pool.close().await;
    }

    fn hidden_link_message(message_id: i32, token: &str) -> Message {
        serde_json::from_value(json!({
            "message_id": message_id,
            "date": 1_700_000_000,
            "chat": {"id": TEST_CHAT_ID, "type": "supergroup"},
            "from": {"id": 7_777_777, "is_bot": false, "first_name": "Synthetic"},
            "text": "open",
            "entities": [{
                "type": "text_link", "offset": 0, "length": 4,
                "url": format!("https://t.me/+{token}")
            }]
        }))
        .unwrap()
    }

    fn reference_url_hash(target: &str) -> String {
        let mut digest = Sha256::new();
        digest.update(b"nedobot-url-evidence-v1\0url\0");
        digest.update(target.as_bytes());
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn photo_message(message_id: i32, file_id: &str, unique_id: &str) -> Message {
        serde_json::from_value(json!({
            "message_id": message_id,
            "date": 1_700_000_000,
            "chat": {"id": TEST_CHAT_ID, "type": "supergroup"},
            "from": {"id": 7_777_777, "is_bot": false, "first_name": "Synthetic"},
            "photo": [{
                "file_id": file_id, "file_unique_id": unique_id,
                "width": 200, "height": 200
            }]
        }))
        .unwrap()
    }

    async fn insert_message(pool: &PgPool, message: &Message) {
        query("insert into telegram_messages (chat_id, message_id, user_id, text, raw_json) values ($1, $2, $3, $4, $5)")
            .bind(message.chat.id.0)
            .bind(message.id.0)
            .bind(message.from.as_ref().map(|user| user.id.0 as i64))
            .bind(message.text())
            .bind(serde_json::to_value(message).unwrap())
            .execute(pool)
            .await
            .unwrap();
    }
}
