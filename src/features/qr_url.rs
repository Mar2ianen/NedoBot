use std::{collections::HashSet, time::Duration};

use sqlx::PgPool;
use teloxide::{net::Download, prelude::*, types::FileId};

use crate::{
    db::url_evidence::{
        MAX_QR_URLS_PER_MESSAGE, QrScanJob, claim_next_qr_scan_job, finish_qr_scan_job,
        replace_qr_evidence_for_job,
    },
    state::AppState,
    telegram::media::LimitedBytesWriter,
};

pub fn spawn_worker(bot: Bot, state: AppState) {
    tokio::spawn(async move {
        loop {
            match process_next_job(&bot, &state.pool).await {
                Ok(true) => continue,
                Ok(false) => tokio::time::sleep(Duration::from_secs(3)).await,
                Err(error_kind) => {
                    tracing::warn!(error_kind, "QR evidence worker failed");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
        }
    });
}

async fn process_next_job(bot: &Bot, pool: &PgPool) -> Result<bool, &'static str> {
    let job = claim_next_qr_scan_job(pool)
        .await
        .map_err(|_| "claim_failed")?;
    let Some(job) = job else {
        return Ok(false);
    };

    let processing =
        tokio::time::timeout(Duration::from_secs(120), process_job(bot, pool, &job)).await;
    match processing {
        Ok(Ok(Some(reference_count))) => {
            finish_qr_scan_job(pool, &job, Some(reference_count), None)
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
            finish_qr_scan_job(pool, &job, None, Some(error_kind))
                .await
                .map_err(|_| "finish_failed")?;
            tracing::warn!(
                job_id = job.id,
                error_kind,
                "QR message evidence scan failed"
            );
        }
        Err(_) => {
            finish_qr_scan_job(pool, &job, None, Some("processing_timeout"))
                .await
                .map_err(|_| "finish_failed")?;
            tracing::warn!(job_id = job.id, "QR message evidence scan timed out");
        }
    }
    Ok(true)
}

async fn process_job(
    bot: &Bot,
    pool: &PgPool,
    job: &QrScanJob,
) -> Result<Option<usize>, &'static str> {
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

    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for payload in &payloads {
        let Some(text) = nedobot_qr::as_utf8(payload) else {
            continue;
        };
        for target in teloxide_url::find_urls(text) {
            if target.kind == teloxide_url::Kind::Url
                && seen.insert(target.value.to_ascii_lowercase())
            {
                urls.push(target.value);
                if urls.len() >= MAX_QR_URLS_PER_MESSAGE {
                    break;
                }
            }
        }
        if urls.len() >= MAX_QR_URLS_PER_MESSAGE {
            break;
        }
    }

    replace_qr_evidence_for_job(pool, job, &urls)
        .await
        .map_err(|_| "evidence_write_failed")
}
