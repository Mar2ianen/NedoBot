use sha2::{Digest, Sha256};
use sqlx::PgPool;
use teloxide::types::Message;
use teloxide_url::{
    Found, Kind, Source, TelegramLink, TelegramPeer, parse_telegram_link, parse_url,
};

pub const URL_EVIDENCE_PARSER_VERSION: i16 = 2;
pub const MAX_QR_URLS_PER_MESSAGE: usize = 64;

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

struct EvidenceRow {
    index: i16,
    kind: &'static str,
    source: &'static str,
    target_sha256: String,
    host: Option<String>,
    display_hint: String,
    privacy_sensitive: bool,
}

fn encode(found: &Found, index: usize) -> anyhow::Result<EvidenceRow> {
    let kind = match found.kind {
        Kind::Url => "url",
        Kind::Mention => "mention",
        Kind::UserId(_) => "user_id",
        Kind::Hashtag => "hashtag",
        Kind::Cashtag => "cashtag",
        Kind::BotCommand => "bot_command",
    };
    let source = source_name(found.source);
    let parsed = (found.kind == Kind::Url)
        .then(|| parse_url(&found.value))
        .flatten();
    let normalized = match found.kind {
        Kind::UserId(id) => format!("telegram-user-id:{id}"),
        _ => parsed
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| found.value.trim().to_lowercase()),
    };
    let mut digest = Sha256::new();
    digest.update(b"nedobot-url-evidence-v1\0");
    digest.update(kind.as_bytes());
    digest.update(b"\0");
    digest.update(normalized.as_bytes());
    let target_sha256 = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    let host = parsed
        .as_ref()
        .and_then(|url| url.host_str())
        .map(str::to_owned);
    let display_hint = parsed.as_ref().map_or_else(
        || format!("{kind}:redacted"),
        |url| match url.host_str() {
            Some(host) => format!("{}://{host}/…", url.scheme()),
            None => format!("{}://…", url.scheme()),
        },
    );
    let telegram_sensitive = parse_telegram_link(&found.value).is_some_and(|link| {
        matches!(
            link,
            TelegramLink::Invite { .. }
                | TelegramLink::AmbiguousPlus { .. }
                | TelegramLink::BotStart { .. }
                | TelegramLink::Share { .. }
                | TelegramLink::Reserved { .. }
                | TelegramLink::Message {
                    peer: TelegramPeer::ChatId(_),
                    ..
                }
        )
    });
    let url_sensitive = parsed
        .as_ref()
        .is_some_and(|url| url.path() != "/" || url.query().is_some());

    Ok(EvidenceRow {
        index: i16::try_from(index)?,
        kind,
        source,
        target_sha256,
        host,
        display_hint,
        privacy_sensitive: telegram_sensitive || url_sensitive,
    })
}

#[cfg(test)]
mod target_identity_tests {
    use teloxide_url::{Found, Kind};

    use super::encode;

    #[test]
    fn native_user_mentions_hash_the_telegram_id_not_the_visible_label() {
        let alice_42 = encode(&Found::new("Alice", Kind::UserId(42)), 0).unwrap();
        let alice_43 = encode(&Found::new("Alice", Kind::UserId(43)), 0).unwrap();
        let different_label_same_id = encode(&Found::new("A.", Kind::UserId(42)), 0).unwrap();

        assert_ne!(alice_42.target_sha256, alice_43.target_sha256);
        assert_eq!(
            alice_42.target_sha256,
            different_label_same_id.target_sha256
        );
    }
}

fn source_name(source: Source) -> &'static str {
    match source {
        Source::VisibleText => "visible_text",
        Source::TelegramEntity => "telegram_entity",
        Source::RichMessage => "rich_message",
        Source::Qr => "qr",
    }
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
    let mut tx = pool.begin().await?;
    let total_count = materialize_message_url_evidence(&mut tx, message, qr_urls).await?;
    tx.commit().await?;
    Ok(total_count)
}

/// Persists QR results only while this exact media revision still owns the lease.
/// The lease row and current Telegram message row remain locked until evidence commits.
pub async fn replace_qr_evidence_for_job(
    pool: &PgPool,
    job: &QrScanJob,
    qr_urls: &[String],
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

    let raw_json: Option<serde_json::Value> = sqlx::query_scalar(
        "select raw_json from public.telegram_messages where chat_id = $1 and message_id = $2 for update",
    )
    .bind(job.chat_id)
    .bind(job.message_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(raw_json) = raw_json else {
        tx.rollback().await?;
        return Ok(None);
    };
    let message: Message = serde_json::from_value(raw_json)?;
    let current_file_id = qr_media_candidate(&message).map(|candidate| candidate.file_unique_id);
    if current_file_id.as_deref() != Some(job.file_unique_id.as_str()) {
        tx.rollback().await?;
        return Ok(None);
    }

    let total_count = materialize_message_url_evidence(&mut tx, &message, Some(qr_urls)).await?;
    tx.commit().await?;
    Ok(Some(total_count))
}

async fn materialize_message_url_evidence(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    message: &Message,
    qr_urls: Option<&[String]>,
) -> anyhow::Result<usize> {
    let mut found = teloxide_url::adapter::extract_from_message(message);
    if let Some(qr_urls) = qr_urls {
        for text in qr_urls.iter().take(MAX_QR_URLS_PER_MESSAGE) {
            for mut url in teloxide_url::find_urls(text) {
                if found
                    .iter()
                    .filter(|item| item.source == Source::Qr)
                    .count()
                    >= MAX_QR_URLS_PER_MESSAGE
                {
                    break;
                }
                url.source = Source::Qr;
                if !found.iter().any(|known| {
                    known.source == Source::Qr && known.value.eq_ignore_ascii_case(&url.value)
                }) {
                    found.push(url);
                }
            }
        }
    }
    found.truncate(teloxide_url::MAX_ITEMS + MAX_QR_URLS_PER_MESSAGE);
    let mut source_indices = std::collections::HashMap::<&'static str, usize>::new();
    let rows = found
        .iter()
        .map(|item| {
            let source = source_name(item.source);
            let index = source_indices.get(source).copied().unwrap_or_default();
            let row = encode(item, index)?;
            *source_indices.entry(source).or_default() += 1;
            Ok(row)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let mut delete = sqlx::query(
        "delete from public.telegram_message_url_evidence where chat_id = $1 and message_id = $2 and parser_version <= $3",
    );
    if qr_urls.is_none() {
        delete = sqlx::query(
            "delete from public.telegram_message_url_evidence where chat_id = $1 and message_id = $2 and parser_version <= $3 and (parser_version < $3 or source <> 'qr')",
        );
    }
    delete
        .bind(message.chat.id.0)
        .bind(message.id.0)
        .bind(URL_EVIDENCE_PARSER_VERSION)
        .execute(&mut **tx)
        .await?;

    for row in &rows {
        sqlx::query(
            r#"insert into public.telegram_message_url_evidence
                (chat_id, message_id, parser_version, item_index, kind, source,
                 target_sha256, host, display_hint, privacy_sensitive)
               values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
               on conflict do nothing"#,
        )
        .bind(message.chat.id.0)
        .bind(message.id.0)
        .bind(URL_EVIDENCE_PARSER_VERSION)
        .bind(row.index)
        .bind(row.kind)
        .bind(row.source)
        .bind(&row.target_sha256)
        .bind(&row.host)
        .bind(&row.display_hint)
        .bind(row.privacy_sensitive)
        .execute(&mut **tx)
        .await?;
    }
    let qr_hashes: Vec<String> = sqlx::query_scalar(
        "select coalesce(array_agg(target_sha256::text order by item_index), array[]::text[]) from public.telegram_message_url_evidence where chat_id = $1 and message_id = $2 and parser_version = $3 and source = 'qr'",
    )
    .bind(message.chat.id.0)
    .bind(message.id.0)
    .bind(URL_EVIDENCE_PARSER_VERSION)
        .fetch_one(&mut **tx)
    .await?;
    let source_fingerprint = source_fingerprint(message, &qr_hashes)?;
    let total_count: i16 = sqlx::query_scalar(
        "select count(*)::smallint from public.telegram_message_url_evidence where chat_id = $1 and message_id = $2 and parser_version = $3",
    )
    .bind(message.chat.id.0)
    .bind(message.id.0)
    .bind(URL_EVIDENCE_PARSER_VERSION)
        .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        r#"insert into public.telegram_message_url_evidence_runs
              (chat_id, message_id, parser_version, source_fingerprint, target_count, processed_at)
           values ($1, $2, $3, $4, $5, now())
           on conflict (chat_id, message_id, parser_version) do update
             set source_fingerprint = excluded.source_fingerprint,
                 target_count = excluded.target_count,
                 processed_at = excluded.processed_at"#,
    )
    .bind(message.chat.id.0)
    .bind(message.id.0)
    .bind(URL_EVIDENCE_PARSER_VERSION)
    .bind(source_fingerprint)
    .bind(total_count)
    .execute(&mut **tx)
    .await?;
    Ok(usize::try_from(total_count)?)
}

fn source_fingerprint(message: &Message, qr_hashes: &[String]) -> anyhow::Result<String> {
    let bytes = serde_json::to_vec(message)?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    for hash in qr_hashes {
        digest.update(b"\0qr:");
        digest.update(hash.as_bytes());
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
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
        let Ok(message) = serde_json::from_value::<Message>(raw_json) else {
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
    use sqlx::{PgPool, query, query_as, query_scalar};

    const TEST_CHAT_ID: i64 = -100_991_223_441;
    const TEST_MESSAGE_ID: i32 = 9_912_234;
    const TEST_LINKED_SOURCE_CHANNEL_ID: i64 = -100_991_223_442;

    #[test]
    fn evidence_redacts_invite_payload_but_keeps_host_and_fingerprint() {
        let found = Found {
            value: "https://t.me/+SYNTHETIC_SECRET_INVITE".into(),
            kind: Kind::Url,
            source: Source::TelegramEntity,
            span: None,
            label: Some("join".into()),
        };
        let row = encode(&found, 0).unwrap();
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
            encode(&a, 0).unwrap().target_sha256,
            encode(&b, 0).unwrap().target_sha256
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
        let row = encode(&found[0], 0).unwrap();
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
            .bind(TEST_MESSAGE_ID + 1)
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
            &job,
            &["https://example.invalid/qr-target".to_owned()],
        )
        .await
        .unwrap();
        assert_eq!(stored, Some(1));
        finish_qr_scan_job(&pool, &job, Some(0), None)
            .await
            .unwrap();
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
            &stale_after_edit,
            &["https://example.invalid/stale-qr".to_owned()],
        )
        .await
        .unwrap();
        assert_eq!(stale_write, None);
        let stale_qr_rows: i64 = query_scalar(
            "select count(*) from telegram_message_url_evidence where chat_id = $1 and message_id = $2 and source = 'qr'",
        )
        .bind(TEST_CHAT_ID)
        .bind(TEST_MESSAGE_ID + 3)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stale_qr_rows, 0);

        query("delete from telegram_messages where chat_id = $1 and message_id between $2 and $3")
            .bind(TEST_CHAT_ID)
            .bind(TEST_MESSAGE_ID)
            .bind(TEST_MESSAGE_ID + 3)
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
