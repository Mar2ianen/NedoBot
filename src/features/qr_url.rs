use std::{collections::HashSet, time::Duration};

use teloxide::{net::Download, prelude::*, types::FileId};

use crate::{
    db::url_evidence::{
        MAX_QR_URLS_PER_MESSAGE, QrScanJob, QrUrlEvidence, claim_next_qr_scan_job,
        finish_qr_scan_job, replace_qr_evidence_for_job,
    },
    features::jobs::{
        policy::WorkerPollPolicy,
        worker::{PollOutcome, WorkerId, WorkerSpec, WorkerSupervisor, run_poll_worker},
    },
    state::AppState,
    telegram::media::LimitedBytesWriter,
};

pub fn spawn_worker(supervisor: &mut WorkerSupervisor, bot: Bot, state: AppState) {
    supervisor.spawn(WorkerId::QrEvidence, move |shutdown| async move {
        run_poll_worker(
            WorkerSpec::new(WorkerId::QrEvidence, WorkerPollPolicy::new(3, 3)),
            shutdown,
            || async {
                match process_next_job(&bot, &state).await {
                    Ok(true) => PollOutcome::Processed,
                    Ok(false) => PollOutcome::Idle,
                    Err(error_kind) => PollOutcome::Failed(error_kind),
                }
            },
        )
        .await;
    });
}

async fn process_next_job(bot: &Bot, state: &AppState) -> Result<bool, &'static str> {
    let job = claim_next_qr_scan_job(&state.pool)
        .await
        .map_err(|_| "claim_failed")?;
    let Some(job) = job else {
        return Ok(false);
    };

    let processing =
        tokio::time::timeout(Duration::from_secs(120), process_job(bot, state, &job)).await;
    match processing {
        Ok(Ok(Some(reference_count))) => {
            finish_qr_scan_job(&state.pool, &job, Some(reference_count), None)
                .await
                .map_err(|_| "finish_failed")?;
            tracing::debug!(
                job_id = job.id,
                reference_count,
                "QR message evidence scanned"
            );
        }
        Ok(Ok(None)) => {
            tracing::debug!(
                job_id = job.id,
                "QR scan result discarded after message or lease changed"
            );
        }
        Ok(Err(error_kind)) => {
            finish_qr_scan_job(&state.pool, &job, None, Some(error_kind))
                .await
                .map_err(|_| "finish_failed")?;
            tracing::warn!(
                job_id = job.id,
                error_kind,
                "QR message evidence scan failed"
            );
        }
        Err(_) => {
            finish_qr_scan_job(&state.pool, &job, None, Some("processing_timeout"))
                .await
                .map_err(|_| "finish_failed")?;
            tracing::warn!(job_id = job.id, "QR message evidence scan timed out");
        }
    }
    Ok(true)
}

async fn process_job(
    bot: &Bot,
    state: &AppState,
    job: &QrScanJob,
) -> Result<Option<usize>, &'static str> {
    let pool = &state.pool;
    let file = bot
        .get_file(FileId(job.file_id.clone()))
        .await
        .map_err(|_| "telegram_api")?;
    if usize::try_from(file.size).unwrap_or(usize::MAX) > nedobot_qr::MAX_COMPRESSED_BYTES {
        return Err("file_too_large");
    }

    let mut writer = LimitedBytesWriter::new(nedobot_qr::MAX_COMPRESSED_BYTES);
    bot.download_file(&file.path, &mut writer)
        .await
        .map_err(|_| "download_failed")?;
    let bytes = writer.into_inner();
    let payloads = tokio::task::spawn_blocking(move || nedobot_qr::scan_image(&bytes))
        .await
        .map_err(|_| "decoder_join_failed")?
        .map_err(|_| "image_decode_failed")?;

    let urls = qr_urls_from_payloads(&payloads);

    let affiliated =
        crate::features::url_relations::load_affiliated_peer_ids(pool, &state.config, job.chat_id)
            .await
            .map_err(|_| "affiliated_channels_lookup_failed")?;
    let classified_urls = urls
        .into_iter()
        .map(|target| QrUrlEvidence {
            relation: crate::features::url_relations::classify_qr_url(
                &target,
                job.chat_id,
                &affiliated,
            ),
            target,
        })
        .collect::<Vec<_>>();
    let materialized = replace_qr_evidence_for_job(pool, job, &classified_urls)
        .await
        .map_err(|_| "evidence_write_failed")?;
    if materialized.is_some() {
        let user_id: Option<i64> = sqlx::query_scalar(
            "select user_id from public.telegram_messages where chat_id = $1 and message_id = $2",
        )
        .bind(job.chat_id)
        .bind(job.message_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| "message_lookup_failed")?
        .flatten();
        if let Some(user_id) = user_id {
            let url_feed = state.url_feed_snapshot.read().await.clone();
            if let Err(error) =
                crate::features::new_user_analysis::refresh_url_shadow_observation_from_links(
                    pool,
                    &state.config,
                    job.chat_id,
                    user_id,
                    url_feed.as_deref(),
                )
                .await
            {
                tracing::warn!(%error, chat_id = job.chat_id, user_id, "failed to refresh URL shadow after QR evidence materialization");
                return Err("shadow_refresh_failed");
            }
        }
    }
    Ok(materialized)
}

fn qr_urls_from_payloads(payloads: &[Vec<u8>]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for payload in payloads {
        let Some(text) = nedobot_qr::as_utf8(payload) else {
            continue;
        };
        for target in teloxide_url::find_urls(text) {
            if target.kind == teloxide_url::Kind::Url {
                let Some(canonical) =
                    teloxide_url::parse_url(&target.value).map(|url| url.to_string())
                else {
                    continue;
                };
                if seen.insert(canonical.clone()) {
                    urls.push(canonical);
                }
                if urls.len() >= MAX_QR_URLS_PER_MESSAGE {
                    break;
                }
            }
        }
        if urls.len() >= MAX_QR_URLS_PER_MESSAGE {
            break;
        }
    }
    urls
}

#[cfg(test)]
mod tests {
    use super::qr_urls_from_payloads;

    #[test]
    fn qr_deduplication_preserves_case_sensitive_path_and_query() {
        let payloads = vec![
            b"https://example.org/File?campaign=Normal".to_vec(),
            b"https://EXAMPLE.org/File?campaign=Normal".to_vec(),
            b"https://example.org/file?campaign=Normal".to_vec(),
            b"https://example.org/File?campaign=normal".to_vec(),
        ];
        assert_eq!(
            qr_urls_from_payloads(&payloads),
            [
                "https://example.org/File?campaign=Normal",
                "https://example.org/file?campaign=Normal",
                "https://example.org/File?campaign=normal",
            ]
        );
    }
}
