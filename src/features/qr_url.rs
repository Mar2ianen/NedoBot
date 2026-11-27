use std::{
    collections::HashSet,
    sync::{Arc, LazyLock},
    time::Duration,
};

use teloxide::{
    adaptors::DefaultParseMode,
    net::Download,
    prelude::*,
    types::{FileId, Message},
};

use crate::{
    db::url_evidence::{
        MAX_QR_URLS_PER_MESSAGE, QrScanJob, QrUrlEvidence, UrlShadowRefreshJob,
        claim_next_qr_scan_job, claim_next_url_shadow_refresh_job, finish_qr_scan_job,
        finish_url_shadow_refresh_job, replace_qr_evidence_for_job,
    },
    features::jobs::{
        policy::WorkerPollPolicy,
        worker::{PollOutcome, WorkerId, WorkerSpec, WorkerSupervisor, run_poll_worker},
    },
    state::AppState,
    telegram::media::LimitedBytesWriter,
};

const QR_SCAN_TIMEOUT: Duration = Duration::from_secs(120);
const QR_COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const QR_FRESHNESS_TIMEOUT: Duration = Duration::from_secs(2);
const QR_COMMAND_MAX_PAYLOADS: usize = 8;
const QR_COMMAND_MAX_DISPLAY_UTF16: usize = 3_200;
const QR_REPORT_SUFFIX_RESERVE_UTF16: usize = 160;
const QR_COMMAND_CHAT_COOLDOWN: Duration = Duration::from_secs(5);
const QR_COMMAND_RATE_LIMIT_ENTRIES: usize = 512;
static QR_SCAN_CPU_SLOT: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(1)));
static QR_COMMAND_DOWNLOAD_SLOTS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));
static QR_COMMAND_LAST_REQUEST: LazyLock<
    tokio::sync::Mutex<std::collections::HashMap<i64, tokio::time::Instant>>,
> = LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QrDownloadError {
    TelegramApi,
    Download,
    FileTooLarge,
    DecoderJoin,
    Decode,
}

impl QrDownloadError {
    fn worker_reason(self) -> &'static str {
        match self {
            Self::TelegramApi => "telegram_api",
            Self::Download => "download_failed",
            Self::FileTooLarge => "file_too_large",
            Self::DecoderJoin => "decoder_join_failed",
            Self::Decode => "image_decode_failed",
        }
    }
}

async fn download_and_decode_payloads(
    bot: &Bot,
    file_id: &str,
) -> Result<Vec<Vec<u8>>, QrDownloadError> {
    let file = bot
        .get_file(FileId(file_id.to_owned()))
        .await
        .map_err(|_| QrDownloadError::TelegramApi)?;
    if usize::try_from(file.size).unwrap_or(usize::MAX) > nedobot_qr::MAX_COMPRESSED_BYTES {
        return Err(QrDownloadError::FileTooLarge);
    }
    let mut writer = LimitedBytesWriter::new(nedobot_qr::MAX_COMPRESSED_BYTES);
    bot.download_file(&file.path, &mut writer)
        .await
        .map_err(|_| QrDownloadError::Download)?;
    run_bounded_qr_decode(writer.into_inner())
        .await
        .map_err(|error| {
            if error == "decoder_join_failed" {
                QrDownloadError::DecoderJoin
            } else {
                QrDownloadError::Decode
            }
        })
}

pub async fn handle_qr_command(
    bot: &DefaultParseMode<Bot>,
    command: &Message,
    state: &AppState,
    audience: Option<crate::telegram::service_messages::MessageAudience>,
) -> ResponseResult<()> {
    use crate::telegram::service_messages::{self, MessageAudience};

    let Some(audience) = audience else {
        return Ok(());
    };
    if !state.config.community.moderation.qr_evidence_enabled {
        send_qr_status(
            bot,
            command,
            "QR-сканирование выключено в настройках этого экземпляра.",
            audience,
        )
        .await?;
        return Ok(());
    }
    let Some(target) = command.reply_to_message() else {
        send_qr_status(
            bot,
            command,
            "Ответьте командой /qr на сообщение с изображением.",
            audience,
        )
        .await?;
        return Ok(());
    };
    if target.chat.id != command.chat.id {
        send_qr_status(
            bot,
            command,
            "QR можно читать только из сообщения этого чата.",
            audience,
        )
        .await?;
        return Ok(());
    }
    if target.has_protected_content() {
        send_qr_status(
            bot,
            command,
            "Нельзя сканировать защищённое содержимое.",
            audience,
        )
        .await?;
        return Ok(());
    }
    let Some(candidate) = crate::db::url_evidence::qr_media_candidate(target) else {
        send_qr_status(
            bot,
            command,
            "В reply нет поддерживаемого изображения.",
            audience,
        )
        .await?;
        return Ok(());
    };
    if !admit_qr_command(command.chat.id.0).await {
        send_qr_status(
            bot,
            command,
            "QR-команду можно запускать не чаще одного раза в 5 секунд на чат.",
            audience,
        )
        .await?;
        return Ok(());
    }
    let Some(download_slot) = try_acquire_qr_command_slot(&QR_COMMAND_DOWNLOAD_SLOTS) else {
        send_qr_status(bot, command, "Сканер QR занят. Повторите позже.", audience).await?;
        return Ok(());
    };
    match tokio::time::timeout(
        QR_FRESHNESS_TIMEOUT,
        qr_media_is_current(&state.pool, target),
    )
    .await
    {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => {
            drop(download_slot);
            send_qr_status(
                bot,
                command,
                "Сообщение отсутствует в хранилище или медиа уже изменилось; скачивание отменено.",
                audience,
            )
            .await?;
            return Ok(());
        }
        Ok(Err(_)) => {
            drop(download_slot);
            send_qr_status(
                bot,
                command,
                "Не удалось проверить медиа в базе; скачивание отменено.",
                audience,
            )
            .await?;
            return Ok(());
        }
        Err(_) => {
            drop(download_slot);
            send_qr_status(
                bot,
                command,
                "Проверка актуальности медиа превысила лимит времени; скачивание отменено.",
                audience,
            )
            .await?;
            return Ok(());
        }
    }
    let decoded = tokio::time::timeout(
        QR_COMMAND_TIMEOUT,
        download_and_decode_payloads(bot.inner(), &candidate.file_id),
    )
    .await;
    let payloads = match decoded {
        Err(_) => {
            drop(download_slot);
            send_qr_status(
                bot,
                command,
                "Сканирование QR превысило лимит времени. Повторите позже.",
                audience,
            )
            .await?;
            return Ok(());
        }
        Ok(Ok(payloads)) => payloads,
        Ok(Err(QrDownloadError::FileTooLarge)) => {
            drop(download_slot);
            send_qr_status(
                bot,
                command,
                "Изображение превышает допустимый размер.",
                audience,
            )
            .await?;
            return Ok(());
        }
        Ok(Err(error)) => {
            drop(download_slot);
            send_qr_status(
                bot,
                command,
                format!("QR не удалось прочитать ({error:?})."),
                audience,
            )
            .await?;
            return Ok(());
        }
    };
    match tokio::time::timeout(
        QR_FRESHNESS_TIMEOUT,
        qr_media_is_current(&state.pool, target),
    )
    .await
    {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => {
            drop(download_slot);
            send_qr_status(
                bot,
                command,
                "Сообщение изменилось или медиа больше неактуально; результат скрыт.",
                audience,
            )
            .await?;
            return Ok(());
        }
        Ok(Err(_)) | Err(_) => {
            drop(download_slot);
            send_qr_status(
                bot,
                command,
                "Не удалось повторно проверить актуальность сообщения; результат не показан.",
                audience,
            )
            .await?;
            return Ok(());
        }
    }
    let report = format_qr_payload_report(&payloads);
    drop(payloads);
    drop(download_slot);
    // The report is plain text; link-like and control-bearing payloads use
    // reversible hex. Empty entities and no parse mode prevent markup parsing.
    service_messages::send_entities(bot, command, &report, Vec::new(), audience).await?;
    if matches!(audience, MessageAudience::Public) {
        tracing::debug!(
            chat_id = command.chat.id.0,
            message_id = target.id.0,
            "QR command returned bounded raw payload report"
        );
    }
    Ok(())
}

async fn send_qr_status(
    bot: &DefaultParseMode<Bot>,
    command: &Message,
    text: impl Into<String>,
    audience: crate::telegram::service_messages::MessageAudience,
) -> ResponseResult<()> {
    let text = text.into();
    crate::telegram::service_messages::send_entities(bot, command, &text, Vec::new(), audience)
        .await?;
    Ok(())
}

async fn admit_qr_command(chat_id: i64) -> bool {
    let now = tokio::time::Instant::now();
    let mut requests = QR_COMMAND_LAST_REQUEST.lock().await;
    requests.retain(|_, timestamp| now.duration_since(*timestamp) < Duration::from_secs(600));
    if let Some(previous) = requests.get(&chat_id)
        && now.duration_since(*previous) < QR_COMMAND_CHAT_COOLDOWN
    {
        return false;
    }
    if requests.len() >= QR_COMMAND_RATE_LIMIT_ENTRIES && !requests.contains_key(&chat_id) {
        return false;
    }
    requests.insert(chat_id, now);
    true
}

fn try_acquire_qr_command_slot(
    slots: &Arc<tokio::sync::Semaphore>,
) -> Option<tokio::sync::OwnedSemaphorePermit> {
    Arc::clone(slots).try_acquire_owned().ok()
}

async fn qr_media_is_current(pool: &sqlx::PgPool, snapshot: &Message) -> Result<bool, sqlx::Error> {
    if snapshot.has_protected_content() {
        return Ok(false);
    }
    if crate::db::url_evidence::qr_media_candidate(snapshot).is_none() {
        return Ok(false);
    }
    let stored: Option<serde_json::Value> = sqlx::query_scalar(
        "select raw_json from public.telegram_messages where chat_id = $1 and message_id = $2",
    )
    .bind(snapshot.chat.id.0)
    .bind(snapshot.id.0)
    .fetch_optional(pool)
    .await?;
    is_stored_media_snapshot_current(snapshot, stored)
        .map_err(|error| sqlx::Error::Decode(Box::new(error)))
}

fn is_stored_media_snapshot_current(
    snapshot: &Message,
    stored: Option<serde_json::Value>,
) -> Result<bool, serde_json::Error> {
    if snapshot.has_protected_content() {
        return Ok(false);
    }
    let Some(stored) = stored else {
        return Ok(false);
    };
    let current: Message = serde_json::from_value(stored)?;
    let Some(snapshot_media) = crate::db::url_evidence::qr_media_candidate(snapshot) else {
        return Ok(false);
    };
    Ok(!current.has_protected_content()
        && current.chat.id == snapshot.chat.id
        && current.id == snapshot.id
        && crate::db::url_evidence::qr_media_candidate(&current)
            .is_some_and(|media| media.file_unique_id == snapshot_media.file_unique_id))
}

fn format_qr_payload_report(payloads: &[Vec<u8>]) -> String {
    if payloads.is_empty() {
        return "В изображении QR-коды не найдены.".to_owned();
    }
    let mut lines = Vec::new();
    let mut omitted = payloads.len().saturating_sub(QR_COMMAND_MAX_PAYLOADS);
    for (index, payload) in payloads.iter().take(QR_COMMAND_MAX_PAYLOADS).enumerate() {
        let entry = match std::str::from_utf8(payload) {
            Ok(text) => {
                let link_like = contains_link_like_text(text);
                if link_like {
                    format!(
                        "QR {} — link-like UTF-8 payload (reversible hex; preview disabled): {}",
                        index + 1,
                        encode_hex(payload)
                    )
                } else if has_unsafe_display_control(text) {
                    format!(
                        "QR {} — UTF-8 payload (reversible hex; controls encoded): {}",
                        index + 1,
                        encode_hex(payload)
                    )
                } else {
                    format!("QR {} — UTF-8 payload: {text}", index + 1)
                }
            }
            Err(_) => {
                let shown = payload
                    .iter()
                    .take(64)
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                let suffix = if payload.len() > 64 { "…" } else { "" };
                format!(
                    "QR {} — binary payload (hex, first 64 bytes): {shown}{suffix}",
                    index + 1
                )
            }
        };
        let entry = if entry.encode_utf16().count() > QR_COMMAND_MAX_DISPLAY_UTF16 - 128 {
            format!(
                "QR {} — payload omitted because it is too large to display completely ({} bytes).",
                index + 1,
                payload.len()
            )
        } else {
            entry
        };
        let current_size = lines.join("\n").encode_utf16().count();
        if current_size + usize::from(!lines.is_empty()) + entry.encode_utf16().count()
            > QR_COMMAND_MAX_DISPLAY_UTF16 - QR_REPORT_SUFFIX_RESERVE_UTF16
        {
            omitted += 1;
        } else {
            lines.push(entry);
        }
    }
    if omitted > 0 {
        lines.push(format!("Ещё {omitted} QR payload(s) опущено по лимиту."));
    }
    lines.join("\n")
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn has_unsafe_display_control(text: &str) -> bool {
    text.chars().any(|ch| {
        ch.is_control()
            || matches!(
                ch as u32,
                0x061c | 0x200e..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069
            )
    })
}

fn contains_link_like_text(text: &str) -> bool {
    let normalized = text
        .chars()
        .map(|ch| if is_url_dot(ch) { '.' } else { ch })
        .collect::<String>();
    if teloxide_url::find_urls(&normalized)
        .iter()
        .any(|found| found.kind == teloxide_url::Kind::Url)
    {
        return true;
    }
    let lower = normalized.to_ascii_lowercase();
    if lower.contains("www.") || lower.contains("://") {
        return true;
    }
    if normalized.split_whitespace().any(|token| {
        let Some((scheme, _)) = token.split_once(':') else {
            return false;
        };
        !scheme.is_empty()
            && scheme
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '-'))
    }) {
        return true;
    }
    normalized.split_whitespace().any(|token| {
        token
            .split_once('@')
            .is_some_and(|(_, domain)| domain.contains('.') && !domain.starts_with('.'))
    })
}

fn is_url_dot(character: char) -> bool {
    matches!(
        character,
        '.' | '\u{2024}' | '\u{3002}' | '\u{FF0E}' | '\u{FF61}'
    )
}

pub fn spawn_worker(supervisor: &mut WorkerSupervisor, bot: Bot, state: AppState) {
    let shadow_state = state.clone();
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
    supervisor.spawn(WorkerId::QrShadowRefresh, move |shutdown| async move {
        run_poll_worker(
            WorkerSpec::new(WorkerId::QrShadowRefresh, WorkerPollPolicy::new(3, 3)),
            shutdown,
            || async {
                match process_next_shadow_refresh(&shadow_state).await {
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

    let processing = tokio::time::timeout(QR_SCAN_TIMEOUT, process_job(bot, state, &job)).await;
    match processing {
        Ok(Ok(Some(reference_count))) => {
            tracing::debug!(
                job_id = job.id,
                reference_count,
                "QR message evidence committed with downstream shadow refresh queued"
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
    let payloads = download_and_decode_payloads(bot, &job.file_id)
        .await
        .map_err(QrDownloadError::worker_reason)?;

    let urls = qr_urls_from_payloads(&payloads);

    let affiliated =
        crate::features::url_relations::load_affiliated_peer_ids(pool, &state.config, job.chat_id)
            .await
            .map_err(|_| "affiliated_channels_lookup_failed")?;
    let mut classified_urls = Vec::with_capacity(urls.len());
    for target in urls {
        // Persist the same fresh unique-ownership decision used elsewhere for
        // Telegram username links. The pure classifier would silently turn
        // every `t.me/<name>/...` link into Unresolved.
        let relation = crate::features::url_relations::classify_qr_url_with_aliases(
            pool,
            &target,
            job.chat_id,
            &affiliated,
        )
        .await
        .map_err(|_| "alias_resolution_failed")?;
        classified_urls.push(QrUrlEvidence { relation, target });
    }
    let materialized = replace_qr_evidence_for_job(
        pool,
        &state.config.community.instance.id,
        job,
        &classified_urls,
    )
    .await
    .map_err(|_| "evidence_write_failed")?;
    Ok(materialized)
}

async fn run_bounded_qr_decode(bytes: Vec<u8>) -> Result<Vec<Vec<u8>>, &'static str> {
    run_bounded_qr_decode_with(Arc::clone(&QR_SCAN_CPU_SLOT), move || {
        nedobot_qr::scan_image(&bytes)
    })
    .await
    .map_err(|_| "decoder_join_failed")?
    .map_err(|_| "image_decode_failed")
}

async fn run_bounded_qr_decode_with<F, R>(
    cpu_slot: Arc<tokio::sync::Semaphore>,
    decode: F,
) -> Result<R, tokio::task::JoinError>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let permit = cpu_slot
        .acquire_owned()
        .await
        .expect("QR decoder CPU semaphore is never closed");
    tokio::task::spawn_blocking(move || {
        // A timed-out JoinHandle detaches this blocking task. Keep the permit
        // inside the closure so a later job cannot create unbounded CPU work.
        let _permit = permit;
        decode()
    })
    .await
}

async fn process_next_shadow_refresh(state: &AppState) -> Result<bool, &'static str> {
    let job = claim_next_url_shadow_refresh_job(&state.pool, &state.config.community.instance.id)
        .await
        .map_err(|_| "shadow_claim_failed")?;
    let Some(job) = job else {
        return Ok(false);
    };
    let url_feed = state.url_feed_snapshot.read().await.clone();
    refresh_shadow_for_job(state, &job, url_feed.as_deref()).await
}

async fn refresh_shadow_for_job(
    state: &AppState,
    job: &UrlShadowRefreshJob,
    url_feed: Option<&crate::features::url_feeds::FeedSnapshot>,
) -> Result<bool, &'static str> {
    let result = crate::features::new_user_analysis::refresh_url_shadow_observation_from_links(
        &state.pool,
        &state.config,
        job.chat_id,
        job.telegram_user_id,
        url_feed,
    )
    .await;
    match result {
        Ok(()) => {
            finish_url_shadow_refresh_job(&state.pool, job, None)
                .await
                .map_err(|_| "shadow_finish_failed")?;
            tracing::debug!(job_id = job.id, "URL shadow refresh completed");
        }
        Err(_) => {
            finish_url_shadow_refresh_job(&state.pool, job, Some("refresh_failed"))
                .await
                .map_err(|_| "shadow_finish_failed")?;
            tracing::warn!(
                job_id = job.id,
                attempt = job.attempts,
                "URL shadow refresh will retry"
            );
        }
    }
    Ok(true)
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
    use std::{sync::Arc, time::Duration};

    use axum::{Json, Router, extract::State, routing::post};
    use teloxide::{prelude::RequesterExt, types::ParseMode};

    use super::{
        contains_link_like_text, format_qr_payload_report, is_stored_media_snapshot_current,
        qr_urls_from_payloads, run_bounded_qr_decode_with,
    };

    #[tokio::test]
    #[ignore = "requires a disposable migrated PostgreSQL database"]
    async fn qr_worker_persists_fresh_alias_relation_and_rejects_superseded_media() {
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        use serde_json::json;
        use teloxide::prelude::*;

        const PNG: &[u8] = include_bytes!("../../tests/fixtures/deep_audit_qr.png");
        let pool = sqlx::PgPool::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let mut config = crate::config::test_config();
        let chat = config.discussion_chat_id;
        config.community.instance.id = "qr_worker_fixture".into();
        let state = crate::state::AppState::new(pool.clone(), config);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .route(
                "/botfixture/GetFile",
                post(|| async {
                    Json(
                        json!({"ok":true,"result":{"file_id":"fixture","file_unique_id":"qr-a",
                    "file_size":PNG.len(),"file_path":"fixture.png"}}),
                    )
                }),
            )
            .route("/file/botfixture/fixture.png", get(|| async { PNG }));
        let fake = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let bot = Bot::new("fixture").set_api_url(format!("http://{address}/").parse().unwrap());
        let user_id = 8_975_501_031_i64;
        let name = "fixture_qr_alias";
        let owner = -1_009_755_010_000_i64;
        let fixture_pool = &pool;
        let insert_alias = |id, age| async move {
            sqlx::query("insert into telegram_username_aliases (username_normalized,telegram_peer_id,peer_type,source,observed_at) values ($1,$2,'channel','configured_chat_refresh',now()-$3*interval '1 hour')")
                .bind(name).bind(id).bind(age).execute(fixture_pool).await.unwrap();
        };
        sqlx::query("delete from telegram_username_aliases where username_normalized=$1")
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();
        insert_alias(owner, 0_i32).await;
        for (index, expected) in [
            "external",
            "unresolved",
            "unresolved",
            "affiliated",
            "superseded",
        ]
        .into_iter()
        .enumerate()
        {
            match index {
                1 => insert_alias(chat, 0).await,
                2 => {
                    sqlx::query("update telegram_username_aliases set observed_at=now()-interval '25 hours' where username_normalized=$1")
                        .bind(name).execute(&pool).await.unwrap();
                }
                3 => {
                    sqlx::query("update telegram_username_aliases set observed_at=now() where username_normalized=$1 and telegram_peer_id=$2")
                        .bind(name).bind(chat).execute(&pool).await.unwrap();
                }
                _ => {}
            }
            let message_id = 1_712_340_000 + index as i32;
            let media = if expected == "superseded" {
                "qr-b"
            } else {
                "qr-a"
            };
            let msg = json!({
                "message_id":message_id, "date":chrono::Utc::now().timestamp(),
                "chat":{"id":chat,"type":"supergroup","title":"QR fixture"},
                "from":{"id":user_id,"is_bot":false,"first_name":"Fixture"},
                "photo":[{"file_id":"fixture","file_unique_id":media,"width":185,"height":185}]
            });
            sqlx::query("insert into telegram_messages (chat_id,message_id,user_id,raw_json) values ($1,$2,$3,$4) on conflict (chat_id,message_id) do update set raw_json=excluded.raw_json, created_at=now()")
                .bind(chat).bind(message_id).bind(user_id).bind(msg).execute(&pool).await.unwrap();
            let job = sqlx::query_as::<_, crate::db::url_evidence::QrScanJob>(
                "insert into telegram_qr_scan_jobs (chat_id,message_id,file_id,file_unique_id,status,attempts,lease_generation,lease_expires_at) values ($1,$2,'fixture','qr-a','leased',1,1,now()+interval '10 minutes') on conflict (chat_id,message_id,file_unique_id) do update set status='leased',lease_expires_at=excluded.lease_expires_at returning id,chat_id,message_id,file_id,file_unique_id,attempts,lease_generation",
            ).bind(chat).bind(message_id).fetch_one(&pool).await.unwrap();
            assert_eq!(
                super::process_job(&bot, &state, &job).await,
                Ok((expected != "superseded").then_some(1))
            );
            let relation: Option<String> = sqlx::query_scalar("select qr_relation from telegram_message_url_evidence where chat_id=$1 and message_id=$2 and source='qr'")
                .bind(chat).bind(message_id).fetch_optional(&pool).await.unwrap();
            assert_eq!(
                relation.as_deref(),
                (expected != "superseded").then_some(expected)
            );
            if index == 0 {
                let summary = crate::features::url_relations::recent_link_relationship_summary(
                    &pool,
                    &state.config,
                    chat,
                    user_id,
                    None,
                )
                .await
                .unwrap();
                assert_eq!(summary["external_qr_url_count"], 1);
            }
        }
        fake.abort();
    }

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

    #[test]
    fn qr_command_distinguishes_no_codes_and_encodes_link_like_payloads() {
        assert_eq!(
            format_qr_payload_report(&[]),
            "В изображении QR-коды не найдены."
        );
        let report = format_qr_payload_report(&[
            "hello <script>\u{202e}".as_bytes().to_vec(),
            b"HTTPS://Example.COM/A?x=1".to_vec(),
            b"tg://emoji?id=42".to_vec(),
            "https://example。org/a".as_bytes().to_vec(),
            b"mailto:alice@example.org".to_vec(),
            b"https://127.0.0.1/admin?next=%2F".to_vec(),
            b"tg://resolve?domain=private_channel".to_vec(),
            vec![0xff, 0x01],
        ]);
        assert!(report.contains("UTF-8 payload (reversible hex; controls encoded)"));
        assert!(report.contains(&super::encode_hex(b"HTTPS://Example.COM/A?x=1")));
        assert!(report.contains(&super::encode_hex(b"tg://emoji?id=42")));
        assert!(report.contains(&super::encode_hex("https://example。org/a".as_bytes())));
        assert!(report.contains(&super::encode_hex(b"https://127.0.0.1/admin?next=%2F")));
        assert!(report.contains(&super::encode_hex(b"tg://resolve?domain=private_channel")));
        assert!(report.contains(&super::encode_hex(b"mailto:alice@example.org")));
        assert!(report.contains("binary payload (hex, first 64 bytes): ff01"));

        let bare = format_qr_payload_report(&[
            b"www.example.org/path".to_vec(),
            b"alice@example.org".to_vec(),
        ]);
        assert!(bare.contains(&super::encode_hex(b"www.example.org/path")));
        assert!(bare.contains(&super::encode_hex(b"alice@example.org")));
    }

    #[test]
    fn absent_stored_qr_message_fails_closed_as_unknown_freshness() {
        assert!(!is_stored_media_snapshot_current(&qr_snapshot(), None).unwrap());
    }

    #[test]
    fn qr_report_preserves_complete_short_url_query_in_reversible_hex() {
        let url = "https://example.org/path?campaign=summer&user=alice";
        let report = format_qr_payload_report(&[url.as_bytes().to_vec()]);
        assert!(report.contains(&super::encode_hex(url.as_bytes())));
    }

    #[tokio::test]
    async fn qr_report_uses_the_safe_plain_text_reply_payload_in_the_right_topic() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let captured = Arc::new(tokio::sync::Mutex::new(None));
        let app = Router::new()
            .fallback(post(capture_qr_send))
            .with_state(Arc::clone(&captured));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let bot = teloxide::Bot::new("fixture")
            .set_api_url(format!("http://{address}/").parse().unwrap())
            .parse_mode(ParseMode::Html);
        let command: teloxide::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 9,
            "date": 1,
            "chat": {"id": -1001, "type": "supergroup", "title": "Test"},
            "message_thread_id": 22,
            "text": "/qr"
        }))
        .unwrap();
        let report = format_qr_payload_report(&[
            b"https://example.org/path?campaign=summer&user=alice".to_vec(),
        ]);
        crate::telegram::service_messages::send_entities(
            &bot,
            &command,
            &report,
            Vec::new(),
            crate::telegram::service_messages::MessageAudience::Public,
        )
        .await
        .unwrap();
        let payload = captured.lock().await.clone().unwrap();
        assert_eq!(payload["text"], report);
        assert_eq!(payload["entities"], serde_json::json!([]));
        assert!(payload.get("parse_mode").is_none());
        assert_eq!(payload["reply_parameters"]["message_id"], 9);
        assert!(
            payload["reply_parameters"]
                .get("allow_sending_without_reply")
                .is_none()
        );
        assert_eq!(payload["message_thread_id"], 22);
        server.abort();
    }

    async fn capture_qr_send(
        State(captured): State<Arc<tokio::sync::Mutex<Option<serde_json::Value>>>>,
        Json(payload): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        let text = payload["text"].as_str().unwrap().to_owned();
        *captured.lock().await = Some(payload);
        Json(serde_json::json!({"ok":true,"result":{
            "message_id":10,
            "date":1,
            "chat":{"id":-1001,"type":"supergroup","title":"Test"},
            "text":text,
            "entities":[]
        }}))
    }

    #[test]
    fn qr_freshness_rejects_changed_media_and_newly_protected_content() {
        let snapshot = qr_snapshot();
        let current = serde_json::json!({
            "message_id": 7, "date": 1,
            "chat": {"id": 42, "type": "private"},
            "photo": [{"file_id":"file", "file_unique_id":"snapshot", "width":1, "height":1}]
        });
        assert!(is_stored_media_snapshot_current(&snapshot, Some(current)).unwrap());

        let changed = serde_json::json!({
            "message_id": 7, "date": 1,
            "chat": {"id": 42, "type": "private"},
            "photo": [{"file_id":"new-file", "file_unique_id":"new-snapshot", "width":1, "height":1}]
        });
        assert!(!is_stored_media_snapshot_current(&snapshot, Some(changed)).unwrap());

        let protected = serde_json::json!({
            "message_id": 7, "date": 1, "has_protected_content": true,
            "chat": {"id": 42, "type": "private"},
            "photo": [{"file_id":"file", "file_unique_id":"snapshot", "width":1, "height":1}]
        });
        assert!(!is_stored_media_snapshot_current(&snapshot, Some(protected)).unwrap());

        let protected_snapshot: teloxide::types::Message =
            serde_json::from_value(serde_json::json!({
                "message_id": 7, "date": 1, "has_protected_content": true,
                "chat": {"id": 42, "type": "private"},
                "photo": [{"file_id":"file", "file_unique_id":"snapshot", "width":1, "height":1}]
            }))
            .unwrap();
        let stored_same_media = serde_json::json!({
            "message_id": 7, "date": 1,
            "chat": {"id": 42, "type": "private"},
            "photo": [{"file_id":"file", "file_unique_id":"snapshot", "width":1, "height":1}]
        });
        assert!(
            !is_stored_media_snapshot_current(&protected_snapshot, Some(stored_same_media))
                .unwrap()
        );

        let wrong_chat = serde_json::json!({
            "message_id": 7, "date": 1,
            "chat": {"id": 43, "type": "private"},
            "photo": [{"file_id":"file", "file_unique_id":"snapshot", "width":1, "height":1}]
        });
        assert!(!is_stored_media_snapshot_current(&snapshot, Some(wrong_chat)).unwrap());
    }

    fn qr_snapshot() -> teloxide::types::Message {
        serde_json::from_value(serde_json::json!({
            "message_id": 7, "date": 1,
            "chat": {"id": 42, "type": "private"},
            "photo": [{"file_id":"file", "file_unique_id":"snapshot", "width":1, "height":1}]
        }))
        .unwrap()
    }

    #[test]
    fn link_like_detection_covers_schemes_bare_hosts_and_email() {
        for text in [
            "tg://emoji?id=7",
            "tg://resolve?domain=channel",
            "mailto:a@example.org",
            "www.example.org/a",
            "https://example。org/a",
            "https://example․org/a",
            "https://127.0.0.1/admin",
            "alice@example.org",
            "https://example.org",
        ] {
            assert!(contains_link_like_text(text), "{text}");
            let encoded = super::encode_hex(text.as_bytes());
            assert!(!encoded.contains("://"));
            assert!(!encoded.contains('@'));
            assert!(!encoded.contains("example.org"));
            let decoded = encoded
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    let digit = |byte: u8| match byte {
                        b'0'..=b'9' => byte - b'0',
                        b'a'..=b'f' => byte - b'a' + 10,
                        _ => panic!("invalid hex digit"),
                    };
                    digit(pair[0]) * 16 + digit(pair[1])
                })
                .collect::<Vec<_>>();
            assert_eq!(decoded, text.as_bytes());
        }
    }

    #[test]
    fn link_like_payload_hex_is_injective_for_unicode_punctuation_and_escape_markers() {
        let samples = [
            "https://example.org/a.b",
            "https://example.org/a․b",
            "https://example.org/：",
            "https://example.org/: ",
            "https://example.org/[U+3002]",
            "https://example.org/。",
            "https://example.org/\\u{000A}",
            "https://example.org/\n",
        ];
        let encoded = samples
            .iter()
            .map(|sample| super::encode_hex(sample.as_bytes()))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(encoded.len(), samples.len());
    }

    #[test]
    fn qr_report_caps_payload_count_and_total_size() {
        let payloads = (0..12).map(|_| vec![b'x'; 4096]).collect::<Vec<_>>();
        let report = format_qr_payload_report(&payloads);
        assert!(report.encode_utf16().count() <= super::QR_COMMAND_MAX_DISPLAY_UTF16);
        assert!(report.contains("Ещё 4 QR payload(s) опущено по лимиту."));
    }

    #[test]
    fn link_like_report_uses_reversible_hex_and_reserves_omission_suffix() {
        let payloads = (0..8)
            .map(|index| format!("https://example{index}.org/{}", "x".repeat(480)).into_bytes())
            .collect::<Vec<_>>();
        let report = format_qr_payload_report(&payloads);
        assert!(report.encode_utf16().count() <= super::QR_COMMAND_MAX_DISPLAY_UTF16);
        assert!(report.contains("reversible hex"));
        assert!(report.contains("опущено по лимиту"));
    }

    #[tokio::test]
    async fn qr_command_rate_limit_is_per_chat_and_bounded() {
        let chat_id = i64::MIN + 92_774;
        assert!(super::admit_qr_command(chat_id).await);
        assert!(!super::admit_qr_command(chat_id).await);
        assert!(super::admit_qr_command(chat_id + 1).await);
    }

    #[test]
    fn qr_command_admission_bounds_aggregate_downloads_and_releases_capacity() {
        let slots = Arc::new(tokio::sync::Semaphore::new(2));
        let first = super::try_acquire_qr_command_slot(&slots).unwrap();
        let second = super::try_acquire_qr_command_slot(&slots).unwrap();
        assert!(super::try_acquire_qr_command_slot(&slots).is_none());
        drop(first);
        assert!(super::try_acquire_qr_command_slot(&slots).is_some());
        drop(second);
    }

    #[tokio::test]
    async fn timed_out_decode_keeps_the_single_cpu_slot_until_blocking_work_finishes() {
        let cpu_slot = Arc::new(tokio::sync::Semaphore::new(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let first_slot = Arc::clone(&cpu_slot);
        let first = tokio::spawn(async move {
            tokio::time::timeout(
                Duration::from_millis(20),
                run_bounded_qr_decode_with(first_slot, move || {
                    let _ = started_tx.send(());
                    release_rx.recv().expect("test releases blocking decoder");
                }),
            )
            .await
        });
        started_rx.await.expect("blocking decoder should start");
        assert!(first.await.expect("timeout wrapper should finish").is_err());

        let blocked = tokio::time::timeout(
            Duration::from_millis(20),
            run_bounded_qr_decode_with(Arc::clone(&cpu_slot), || ()),
        )
        .await;
        assert!(
            blocked.is_err(),
            "second decode must wait for the detached task"
        );

        release_tx.send(()).expect("decoder task is still waiting");
        let next = tokio::time::timeout(
            Duration::from_secs(1),
            run_bounded_qr_decode_with(cpu_slot, || 42),
        )
        .await
        .expect("CPU slot should be released when blocking work completes")
        .expect("second decoder task should join");
        assert_eq!(next, 42);
    }
}
