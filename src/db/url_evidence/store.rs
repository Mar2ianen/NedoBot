use super::*;
use teloxide_url::Source;

/// Owns the SQL write boundary for URL evidence materialization.
pub(super) struct UrlEvidenceStore;

impl UrlEvidenceStore {
    pub(super) async fn materialize(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        message: &Message,
        qr_urls: Option<&[QrUrlEvidence]>,
    ) -> anyhow::Result<usize> {
        materialize_message_url_evidence(tx, message, qr_urls).await
    }
}

async fn materialize_message_url_evidence(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    message: &Message,
    qr_urls: Option<&[QrUrlEvidence]>,
) -> anyhow::Result<usize> {
    let mut found = crate::telegram::url_targets::extract_message_evidence(message);
    let mut qr_relations = std::collections::HashMap::<String, &'static str>::new();
    if let Some(qr_urls) = qr_urls {
        for qr_url in qr_urls.iter().take(MAX_QR_URLS_PER_MESSAGE) {
            for mut url in teloxide_url::find_urls(&qr_url.target) {
                if found
                    .iter()
                    .filter(|item| item.source == Source::Qr)
                    .count()
                    >= MAX_QR_URLS_PER_MESSAGE
                {
                    break;
                }
                url.source = Source::Qr;
                let canonical_url = canonical_url_identity(&url.value);
                qr_relations.insert(canonical_url.clone(), qr_url.relation.as_str());
                if !found.iter().any(|known| {
                    known.source == Source::Qr
                        && canonical_url_identity(&known.value) == canonical_url
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
            let row = encode(
                item,
                index,
                (item.source == Source::Qr)
                    .then(|| {
                        qr_relations
                            .get(&canonical_url_identity(&item.value))
                            .copied()
                    })
                    .flatten(),
            )?;
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
                 target_sha256, host, display_hint, privacy_sensitive, qr_relation)
               values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
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
        .bind(row.qr_relation)
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
