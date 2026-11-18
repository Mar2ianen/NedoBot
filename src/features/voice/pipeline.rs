use teloxide::prelude::*;
use teloxide::types::{InputRichMessage, MessageId};

use crate::features::ingest::managed_chat_allows;
use crate::features::voice::asr::transcribe_audio_with_shadow;
use crate::features::voice::cleanup::cleanup_transcript;
use crate::features::voice::download::{download_media_file, validate_media};
use crate::features::voice::render::{RenderedTranscript, render_transcript};
use crate::features::voice::repo::{
    VoiceJob, VoiceJobPhase, VoiceTransition, claim_next_voice_job, enqueue_voice_job,
    mark_voice_job_delivery_unknown, mark_voice_job_failed, mark_voice_job_phase,
    mark_voice_job_retry_or_failed, mark_voice_job_skipped, recover_expired_voice_deliveries,
    save_asr_result, save_progress_message, save_voice_result,
};
use crate::features::voice::types::{AsrTranscript, VoiceEnqueueOutcome, VoiceMedia};
use crate::state::AppState;
use crate::telegram::delivery::{
    BotTelegramSendPort, DeliveryEffect, DeliveryErrorKind, TelegramSendPort,
    classify_request_result,
};
use crate::telegram::render::{normalize_rich_text, send_html_reply};

const NO_SPEECH_MESSAGE: &str = "В записи не нашёл распознаваемой речи — не буду додумывать текст.";

const NO_SPEECH_ARTIFACTS: &[&str] = &[
    "музыка",
    "тишина",
    "звуки музыки",
    "аплодисменты",
    "смех",
    "music",
    "silence",
    "background music",
    "субтитры сделал",
    "субтитры создавал",
    "редактор субтитров",
    "корректор субтитров",
    "продолжение следует",
    "спасибо за просмотр",
    "подписывайтесь на канал",
    "ставьте лайки",
];

pub async fn maybe_transcribe_voice(
    msg: &Message,
    state: &AppState,
) -> anyhow::Result<VoiceEnqueueOutcome> {
    if !state.config.voice.enabled || !state.config.voice.auto_transcribe {
        return Ok(VoiceEnqueueOutcome::NotTriggered);
    }

    if !voice_chat_is_supported(msg, state)
        || msg.from.as_ref().is_some_and(|user| user.is_bot)
        || msg
            .text()
            .is_some_and(|text| text.trim_start().starts_with('/'))
        || msg.is_automatic_forward()
    {
        return Ok(VoiceEnqueueOutcome::NotTriggered);
    }

    transcribe_media_message(msg, state).await
}

pub async fn transcribe_reply(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    command_message: &Message,
    state: &AppState,
) -> anyhow::Result<VoiceEnqueueOutcome> {
    if !state.config.voice.enabled {
        send_voice_command_message(
            bot,
            command_message,
            "Расшифровка голосовых сейчас отключена.",
        )
        .await?;
        return Ok(VoiceEnqueueOutcome::NotTriggered);
    }
    if !voice_chat_is_supported(command_message, state) {
        send_voice_command_message(
            bot,
            command_message,
            "Эта команда доступна только в личном чате или основном чате.",
        )
        .await?;
        return Ok(VoiceEnqueueOutcome::NotTriggered);
    }

    let Some(reply) = command_message.reply_to_message() else {
        send_voice_command_message(
            bot,
            command_message,
            "Ответьте командой /transcribe на voice, audio или кружок.",
        )
        .await?;
        return Ok(VoiceEnqueueOutcome::NotTriggered);
    };
    if VoiceMedia::from_message(reply).is_none() {
        send_voice_command_message(
            bot,
            command_message,
            "Нужен reply на voice, audio или кружок.",
        )
        .await?;
        return Ok(VoiceEnqueueOutcome::NotTriggered);
    }

    transcribe_media_message(reply, state).await
}

fn voice_chat_is_supported(msg: &Message, state: &AppState) -> bool {
    (msg.chat.is_private() && state.config.community.voice.private_enabled)
        || managed_chat_allows(&state.config, msg.chat.id.0, |chat| chat.voice)
}

async fn send_voice_command_message(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    message: &Message,
    text: &str,
) -> anyhow::Result<()> {
    send_html_reply(
        bot,
        message.chat.id,
        message.id,
        crate::telegram::render::escape_html(text),
    )
    .await?;
    Ok(())
}

async fn transcribe_media_message(
    msg: &Message,
    state: &AppState,
) -> anyhow::Result<VoiceEnqueueOutcome> {
    let Some(media) = VoiceMedia::from_message(msg) else {
        return Ok(VoiceEnqueueOutcome::NotTriggered);
    };

    let Some(job_id) = enqueue_voice_job(&state.pool, &media).await? else {
        tracing::debug!(
            chat_id = media.chat_id,
            message_id = media.message_id,
            "voice transcription job already exists; duplicate update was not enqueued"
        );
        return Ok(VoiceEnqueueOutcome::AlreadyPresent);
    };
    Ok(VoiceEnqueueOutcome::Enqueued { job_id })
}

pub async fn process_next_voice_job(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    state: &AppState,
) -> anyhow::Result<bool> {
    let recovered = recover_expired_voice_deliveries(&state.pool).await?;
    if recovered > 0 {
        tracing::warn!(count = recovered, "recovered expired voice delivery leases");
    }
    let Some(job) = claim_next_voice_job(&state.pool).await? else {
        return Ok(false);
    };
    let queue_latency_seconds = chrono::Utc::now()
        .signed_duration_since(job.created_at)
        .num_seconds()
        .max(0);
    tracing::info!(
        job_id = job.id,
        queue_latency_seconds,
        "claimed voice transcription job"
    );
    let media = job.media()?;
    process_claimed_voice_job(bot, state, job, media).await?;
    Ok(true)
}

async fn process_claimed_voice_job(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    state: &AppState,
    job: VoiceJob,
    media: VoiceMedia,
) -> anyhow::Result<()> {
    if let Err(skip) = validate_media(&media, &state.config) {
        let progress_message_id = ensure_progress_message(bot, &state.pool, &job).await?;
        let transition =
            mark_voice_job_failed(&state.pool, &job, "validation_failed", &skip.user_message())
                .await?;
        if transition == VoiceTransition::LeaseLost {
            tracing::warn!(job_id = job.id, "voice validation result lost its lease");
            return Ok(());
        }
        edit_transcription_message(
            bot,
            ChatId(media.chat_id),
            progress_message_id,
            &skip.user_message(),
        )
        .await?;
        return Ok(());
    }

    let progress_message_id = ensure_progress_message(bot, &state.pool, &job).await?;
    if let Err(failure) = process_voice_job(bot, state, &job, &media, progress_message_id).await {
        if matches!(failure, VoiceProcessingFailure::LeaseLost) {
            tracing::warn!(job_id = job.id, "voice transcription lease was lost");
            return Ok(());
        }
        if matches!(failure, VoiceProcessingFailure::DeliveryUnknown) {
            match mark_voice_job_delivery_unknown(
                &state.pool,
                &job,
                "Telegram delivery or DB finalization outcome is unknown",
            )
            .await?
            {
                VoiceTransition::DeliveryUnknown => {
                    tracing::error!(
                        job_id = job.id,
                        "voice delivery outcome is unknown; automatic retry suppressed"
                    );
                }
                VoiceTransition::LeaseLost => {
                    tracing::warn!(
                        job_id = job.id,
                        "voice delivery-unknown transition lost its lease"
                    );
                }
                transition => {
                    tracing::error!(
                        job_id = job.id,
                        ?transition,
                        "unexpected voice delivery-unknown transition"
                    );
                }
            }
            return Ok(());
        }
        tracing::warn!(
            job_id = job.id,
            error_kind = failure.error_kind(),
            "voice transcription failed"
        );
        let transition = mark_voice_job_retry_or_failed(
            &state.pool,
            &job,
            failure.error_kind(),
            failure
                .user_message()
                .unwrap_or("voice transcription failed"),
        )
        .await?;
        if transition == VoiceTransition::LeaseLost {
            tracing::warn!(job_id = job.id, "voice failure result lost its lease");
            return Ok(());
        }
        if let Some(message) = failure.user_message() {
            edit_transcription_message(bot, ChatId(media.chat_id), progress_message_id, message)
                .await?;
        }
    }
    Ok(())
}

async fn ensure_progress_message(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &sqlx::PgPool,
    job: &VoiceJob,
) -> anyhow::Result<MessageId> {
    if let Some(message_id) = job.progress_message_id {
        return Ok(MessageId(message_id));
    }
    let progress = send_html_reply(
        bot,
        ChatId(job.chat_id),
        MessageId(job.message_id),
        "Расшифровка…",
    )
    .await?;
    if !save_progress_message(pool, job, progress.id.0).await? {
        anyhow::bail!("voice transcription lease lost while saving progress message");
    }
    Ok(progress.id)
}

#[derive(Clone, Copy)]
enum VoiceProcessingFailure {
    Download,
    Asr,
    Cleanup,
    DeliveryRejected,
    DeliveryUnknown,
    LeaseLost,
}

impl VoiceProcessingFailure {
    fn error_kind(self) -> &'static str {
        match self {
            Self::Download => "download_failed",
            Self::Asr => "asr_failed",
            Self::Cleanup => "cleanup_failed",
            Self::DeliveryRejected => "delivery_rejected",
            Self::DeliveryUnknown => "delivery_unknown",
            Self::LeaseLost => "lease_lost",
        }
    }

    fn user_message(self) -> Option<&'static str> {
        match self {
            Self::Download => Some("Не смог скачать запись из Telegram. Попробуйте ещё раз позже."),
            Self::Asr => Some(
                "Не смог расшифровать запись: сервис распознавания временно недоступен. Попробуйте позже.",
            ),
            Self::Cleanup => Some("Не смог подготовить расшифровку. Попробуйте позже."),
            Self::DeliveryRejected | Self::DeliveryUnknown => None,
            Self::LeaseLost => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum VoiceDeliveryFailure {
    PayloadRejected,
    Rejected,
    Unknown,
}

fn classify_voice_delivery_effect<T>(effect: &DeliveryEffect<T>) -> VoiceDeliveryFailure {
    match effect {
        DeliveryEffect::NotAttempted(failure)
            if failure.kind == DeliveryErrorKind::InvalidInput =>
        {
            VoiceDeliveryFailure::PayloadRejected
        }
        DeliveryEffect::Rejected(failure) if failure.kind == DeliveryErrorKind::PayloadRejected => {
            VoiceDeliveryFailure::PayloadRejected
        }
        DeliveryEffect::NotAttempted(_) | DeliveryEffect::Rejected(_) => {
            VoiceDeliveryFailure::Rejected
        }
        DeliveryEffect::Unknown(_) => VoiceDeliveryFailure::Unknown,
        DeliveryEffect::Confirmed(_) => unreachable!("confirmed delivery has no failure"),
    }
}

impl From<VoiceDeliveryFailure> for VoiceProcessingFailure {
    fn from(failure: VoiceDeliveryFailure) -> Self {
        match failure {
            VoiceDeliveryFailure::PayloadRejected | VoiceDeliveryFailure::Rejected => {
                Self::DeliveryRejected
            }
            VoiceDeliveryFailure::Unknown => Self::DeliveryUnknown,
        }
    }
}

async fn process_voice_job(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    state: &AppState,
    job: &VoiceJob,
    media: &VoiceMedia,
    progress_message_id: MessageId,
) -> Result<(), VoiceProcessingFailure> {
    mark_voice_job_phase(&state.pool, job, VoiceJobPhase::Downloading)
        .await
        .map_err(|_| VoiceProcessingFailure::Download)?
        .then_some(())
        .ok_or(VoiceProcessingFailure::LeaseLost)?;
    let (transcript, alternatives) = {
        let downloaded = download_media_file(bot, media)
            .await
            .map_err(|_| VoiceProcessingFailure::Download)?;
        tracing::info!(
            job_id = job.id,
            size = downloaded.size,
            media_kind = media.kind.as_str(),
            "downloaded media file for transcription"
        );

        mark_voice_job_phase(&state.pool, job, VoiceJobPhase::Transcribing)
            .await
            .map_err(|_| VoiceProcessingFailure::Asr)?
            .then_some(())
            .ok_or(VoiceProcessingFailure::LeaseLost)?;
        transcribe_audio_with_shadow(
            &state.config,
            &downloaded.path,
            &downloaded.filename,
            downloaded.mime_type.as_deref(),
        )
        .await
        .map_err(|_| VoiceProcessingFailure::Asr)?
    };
    save_asr_result(&state.pool, job, &transcript, &alternatives)
        .await
        .map_err(|_| VoiceProcessingFailure::Asr)?
        .then_some(())
        .ok_or(VoiceProcessingFailure::LeaseLost)?;
    if !transcript_has_speech(&transcript) {
        mark_voice_job_skipped(&state.pool, job, NO_SPEECH_MESSAGE)
            .await
            .map_err(|_| VoiceProcessingFailure::Cleanup)?
            .then_some(())
            .ok_or(VoiceProcessingFailure::LeaseLost)?;
        edit_transcription_message(
            bot,
            ChatId(media.chat_id),
            progress_message_id,
            NO_SPEECH_MESSAGE,
        )
        .await
        .map_err(|_| VoiceProcessingFailure::Cleanup)?;
        return Ok(());
    }

    mark_voice_job_phase(&state.pool, job, VoiceJobPhase::Cleaning)
        .await
        .map_err(|_| VoiceProcessingFailure::Cleanup)?
        .then_some(())
        .ok_or(VoiceProcessingFailure::LeaseLost)?;
    let cleanup = cleanup_transcript(&state.config, &transcript, &alternatives)
        .await
        .map_err(|_| VoiceProcessingFailure::Cleanup)?;
    let rendered = render_transcript(&cleanup.transcript, &state.config);
    mark_voice_job_phase(&state.pool, job, VoiceJobPhase::Delivering)
        .await
        .map_err(|_| VoiceProcessingFailure::DeliveryUnknown)?
        .then_some(())
        .ok_or(VoiceProcessingFailure::LeaseLost)?;
    let sent = send_rendered_transcript(
        bot,
        &BotTelegramSendPort::new(bot),
        ChatId(media.chat_id),
        MessageId(media.message_id),
        progress_message_id,
        &rendered,
    )
    .await
    .map_err(VoiceProcessingFailure::from)?;
    match save_voice_result(
        &state.pool,
        job,
        &cleanup,
        &sent.html,
        sent.file_id.as_deref(),
    )
    .await
    {
        Ok(true) => {}
        Ok(false) | Err(_) => return Err(VoiceProcessingFailure::DeliveryUnknown),
    }

    Ok(())
}

fn transcript_has_speech(transcript: &AsrTranscript) -> bool {
    meaningful_asr_text(&transcript.text)
        || transcript
            .segments
            .iter()
            .any(|segment| meaningful_asr_text(&segment.text))
}

fn meaningful_asr_text(text: &str) -> bool {
    let normalized = normalize_asr_text(text);
    if normalized.chars().filter(|ch| ch.is_alphanumeric()).count() < 2 {
        return false;
    }

    !NO_SPEECH_ARTIFACTS
        .iter()
        .any(|artifact| no_speech_artifact_matches(&normalized, artifact))
}

fn no_speech_artifact_matches(normalized_text: &str, artifact: &str) -> bool {
    let artifact = normalize_asr_text(artifact);
    if artifact.split_whitespace().count() <= 1 {
        return normalized_text == artifact;
    }

    normalized_text == artifact || normalized_text.contains(&artifact)
}

fn normalize_asr_text(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|ch| if ch.is_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

struct SentRenderedTranscript {
    html: String,
    file_id: Option<String>,
}

async fn edit_transcription_message(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    message_id: MessageId,
    text: &str,
) -> ResponseResult<()> {
    bot.edit_message_text(chat_id, message_id, text).await?;
    Ok(())
}

async fn send_rendered_transcript(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    send_port: &impl TelegramSendPort,
    chat_id: ChatId,
    source_message_id: MessageId,
    progress_message_id: MessageId,
    rendered: &RenderedTranscript,
) -> Result<SentRenderedTranscript, VoiceDeliveryFailure> {
    match rendered {
        RenderedTranscript::Message { html } => {
            edit_transcription_message(bot, chat_id, progress_message_id, html)
                .await
                .map_err(|error| {
                    classify_voice_delivery_effect(&classify_request_result::<()>(Err(error)))
                })?;
            Ok(SentRenderedTranscript {
                html: html.clone(),
                file_id: None,
            })
        }
        RenderedTranscript::RichMessage { html, fallback } => {
            edit_transcription_message(
                bot,
                chat_id,
                progress_message_id,
                "Расшифровка готова. Полный текст — следующим сообщением.",
            )
            .await
            .map_err(|error| {
                classify_voice_delivery_effect(&classify_request_result::<()>(Err(error)))
            })?;
            send_rich_transcript_with_fallback(
                bot,
                send_port,
                chat_id,
                source_message_id,
                progress_message_id,
                html,
                fallback,
            )
            .await
        }
        RenderedTranscript::MessageAndFile { .. } => {
            edit_transcription_message(
                bot,
                chat_id,
                progress_message_id,
                "Расшифровка готова. Полный текст — следующим сообщением.",
            )
            .await
            .map_err(|error| {
                classify_voice_delivery_effect(&classify_request_result::<()>(Err(error)))
            })?;
            send_regular_transcript(
                bot,
                send_port,
                chat_id,
                source_message_id,
                progress_message_id,
                rendered,
            )
            .await
        }
    }
}

async fn send_rich_transcript_with_fallback(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    send_port: &impl TelegramSendPort,
    chat_id: ChatId,
    source_message_id: MessageId,
    progress_message_id: MessageId,
    html: &str,
    fallback: &RenderedTranscript,
) -> Result<SentRenderedTranscript, VoiceDeliveryFailure> {
    let effect = match normalize_rich_text(html.to_owned()) {
        Ok(bounded_html) => {
            let rich = InputRichMessage::html(bounded_html).skip_entity_detection(true);
            send_port
                .send_rich_message_reply(chat_id, source_message_id, rich)
                .await
        }
        Err(error) => classify_request_result::<Message>(Err(error)),
    };
    match effect {
        DeliveryEffect::Confirmed(_) => Ok(SentRenderedTranscript {
            html: html.to_owned(),
            file_id: None,
        }),
        effect => match classify_voice_delivery_effect(&effect) {
            VoiceDeliveryFailure::PayloadRejected => {
                send_regular_transcript(
                    bot,
                    send_port,
                    chat_id,
                    source_message_id,
                    progress_message_id,
                    fallback,
                )
                .await
            }
            VoiceDeliveryFailure::Rejected => Err(VoiceDeliveryFailure::Rejected),
            VoiceDeliveryFailure::Unknown => Err(VoiceDeliveryFailure::Unknown),
        },
    }
}

async fn send_regular_transcript(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    send_port: &impl TelegramSendPort,
    chat_id: ChatId,
    source_message_id: MessageId,
    progress_message_id: MessageId,
    rendered: &RenderedTranscript,
) -> Result<SentRenderedTranscript, VoiceDeliveryFailure> {
    match rendered {
        RenderedTranscript::Message { html } => {
            match send_port
                .send_html_reply(chat_id, source_message_id, html.clone())
                .await
            {
                DeliveryEffect::Confirmed(_) => {}
                effect => return Err(classify_voice_delivery_effect(&effect)),
            }
            Ok(SentRenderedTranscript {
                html: html.clone(),
                file_id: None,
            })
        }
        RenderedTranscript::MessageAndFile {
            html,
            filename,
            body,
        } => {
            match send_port
                .send_html_reply(chat_id, source_message_id, html.clone())
                .await
            {
                DeliveryEffect::Confirmed(_) => {}
                effect => return Err(classify_voice_delivery_effect(&effect)),
            }
            let sent = send_port
                .send_document_reply(chat_id, source_message_id, filename.clone(), body.clone())
                .await;
            let sent = match sent {
                DeliveryEffect::Confirmed(sent) => sent,
                effect => {
                    tracing::warn!(
                        certainty = ?effect.certainty(),
                        chat_id = chat_id.0,
                        "full voice transcript file delivery failed after preview was delivered"
                    );
                    if edit_transcription_message(
                        bot,
                        chat_id,
                        progress_message_id,
                        "Не удалось отправить файл расшифровки. Предварительный текст уже отправлен; автоматическая повторная отправка остановлена, чтобы не дублировать сообщение.",
                    )
                    .await
                    .is_err()
                    {
                        tracing::warn!("failed to update voice transcription delivery status");
                    }
                    return Err(VoiceDeliveryFailure::Unknown);
                }
            };
            Ok(SentRenderedTranscript {
                html: html.clone(),
                file_id: sent.document().map(|document| document.file.id.to_string()),
            })
        }
        RenderedTranscript::RichMessage { .. } => Err(VoiceDeliveryFailure::Rejected),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use super::*;
    use crate::features::voice::types::AsrSegment;
    use axum::{Router, body::Bytes, extract::State, routing::post};
    use sqlx::postgres::PgPoolOptions;
    use teloxide::{prelude::RequesterExt, types::ParseMode};

    type CapturedTelegramRequest = Arc<Mutex<Option<String>>>;

    struct FaultInjectingSendPort {
        outcomes: Mutex<VecDeque<DeliveryEffect<Message>>>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl FaultInjectingSendPort {
        fn new(outcomes: impl IntoIterator<Item = DeliveryEffect<Message>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn next(&self, name: &'static str) -> DeliveryEffect<Message> {
            self.calls.lock().unwrap().push(name);
            self.outcomes.lock().unwrap().pop_front().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl TelegramSendPort for FaultInjectingSendPort {
        async fn send_html_reply(
            &self,
            _: ChatId,
            _: MessageId,
            _: String,
        ) -> DeliveryEffect<Message> {
            self.next("html")
        }

        async fn send_rich_message_reply(
            &self,
            _: ChatId,
            _: MessageId,
            _: InputRichMessage,
        ) -> DeliveryEffect<Message> {
            self.next("rich")
        }

        async fn send_document_reply(
            &self,
            _: ChatId,
            _: MessageId,
            _: String,
            _: String,
        ) -> DeliveryEffect<Message> {
            self.next("document")
        }
    }

    fn delivery_failure(kind: DeliveryErrorKind) -> crate::telegram::delivery::DeliveryFailure {
        crate::telegram::delivery::DeliveryFailure {
            kind,
            retry_after_seconds: None,
        }
    }

    fn sent_message() -> Message {
        serde_json::from_value(serde_json::json!({
            "message_id": 77,
            "date": 1_700_000_000,
            "chat": {"id": 10, "type": "private"},
            "text": "sent"
        }))
        .unwrap()
    }

    fn transcript(text: &str, segments: Vec<&str>) -> AsrTranscript {
        AsrTranscript {
            provider: "groq".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
            request_id: None,
            text: text.to_string(),
            segments: segments
                .into_iter()
                .map(|text| AsrSegment {
                    start_sec: 0.0,
                    end_sec: 1.0,
                    text: text.to_string(),
                })
                .collect(),
            raw_json: serde_json::json!({}),
        }
    }

    #[test]
    fn empty_transcript_is_not_speech() {
        assert!(!transcript_has_speech(&transcript("   ", Vec::new())));
    }

    #[test]
    fn voice_delivery_classification_retries_only_confirmed_rejections() {
        assert_eq!(
            classify_voice_delivery_effect(&DeliveryEffect::<()>::Rejected(
                crate::telegram::delivery::DeliveryFailure {
                    kind: DeliveryErrorKind::RateLimited,
                    retry_after_seconds: Some(1),
                }
            )),
            VoiceDeliveryFailure::Rejected
        );
        assert_eq!(
            classify_voice_delivery_effect(&DeliveryEffect::<()>::Rejected(
                crate::telegram::delivery::DeliveryFailure {
                    kind: DeliveryErrorKind::PayloadRejected,
                    retry_after_seconds: None,
                }
            )),
            VoiceDeliveryFailure::PayloadRejected
        );
        assert_eq!(
            classify_voice_delivery_effect(&DeliveryEffect::<()>::Rejected(
                crate::telegram::delivery::DeliveryFailure {
                    kind: DeliveryErrorKind::TelegramRejected,
                    retry_after_seconds: None,
                }
            )),
            VoiceDeliveryFailure::Rejected
        );

        assert_eq!(
            classify_voice_delivery_effect(&DeliveryEffect::<()>::Unknown(
                crate::telegram::delivery::DeliveryFailure {
                    kind: DeliveryErrorKind::Transport,
                    retry_after_seconds: None,
                }
            )),
            VoiceDeliveryFailure::Unknown
        );
    }

    #[tokio::test]
    async fn unknown_rich_send_never_starts_a_second_message_fallback() {
        let port = FaultInjectingSendPort::new([DeliveryEffect::Unknown(delivery_failure(
            DeliveryErrorKind::Transport,
        ))]);
        let bot = Bot::new("123456:TEST_TOKEN").parse_mode(ParseMode::Html);
        let fallback = RenderedTranscript::Message {
            html: "safe fallback".to_owned(),
        };

        let result = send_rich_transcript_with_fallback(
            &bot,
            &port,
            ChatId(-100),
            MessageId(70),
            MessageId(71),
            "<p>full</p>",
            &fallback,
        )
        .await;

        assert!(matches!(result, Err(VoiceDeliveryFailure::Unknown)));
        assert_eq!(*port.calls.lock().unwrap(), ["rich"]);
    }

    #[tokio::test]
    async fn document_failure_after_confirmed_preview_is_unknown_without_redelivery() {
        let captured = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route("/{*path}", post(fake_telegram_edit))
            .with_state(Arc::clone(&captured));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fake Telegram listener must bind");
        let address = listener.local_addr().expect("fake listener address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let bot = Bot::new("123456:TEST_TOKEN")
            .set_api_url(format!("http://{address}/").parse().expect("mock URL"))
            .parse_mode(ParseMode::Html);
        let port = FaultInjectingSendPort::new([
            DeliveryEffect::Confirmed(sent_message()),
            DeliveryEffect::Unknown(delivery_failure(DeliveryErrorKind::Transport)),
        ]);

        let result = send_rendered_transcript(
            &bot,
            &port,
            ChatId(-1001),
            MessageId(11),
            MessageId(27),
            &RenderedTranscript::MessageAndFile {
                html: "preview already delivered".to_owned(),
                filename: "transcript.txt".to_owned(),
                body: "complete transcript".to_owned(),
            },
        )
        .await;
        server.abort();

        assert!(matches!(result, Err(VoiceDeliveryFailure::Unknown)));
        assert_eq!(*port.calls.lock().unwrap(), ["html", "document"]);
        assert!(
            captured.lock().unwrap().is_some(),
            "progress message was updated"
        );
    }

    #[tokio::test]
    #[ignore = "run against the disposable local test database"]
    async fn voice_partial_delivery_is_persisted_and_never_replayed_after_restart() {
        use crate::features::voice::repo::{
            VoiceJobPhase, VoiceTransition, claim_voice_job, enqueue_voice_job,
            mark_voice_job_delivery_unknown, mark_voice_job_phase,
        };
        use crate::features::voice::types::{VoiceMedia, VoiceMediaKind};

        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must point to the disposable test database");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("test PostgreSQL must be reachable");
        let suffix = chrono::Utc::now().timestamp_micros();
        let media = VoiceMedia {
            chat_id: -1001932061163,
            message_id: (suffix.rem_euclid(1_000_000_000) as i32).saturating_add(1_000_000_000),
            user_id: Some(suffix),
            kind: VoiceMediaKind::Voice,
            file_id: format!("partial-delivery-{suffix}"),
            file_unique_id: Some(format!("partial-delivery-unique-{suffix}")),
            duration_sec: Some(60),
            file_size: Some(1024),
            mime_type: Some("audio/ogg".to_owned()),
        };
        let job_id = enqueue_voice_job(&pool, &media)
            .await
            .expect("voice job must enqueue")
            .expect("new voice job must be returned");
        let job = claim_voice_job(&pool, job_id)
            .await
            .expect("voice claim must succeed")
            .expect("job must be claimed");
        assert!(
            mark_voice_job_phase(&pool, &job, VoiceJobPhase::Delivering)
                .await
                .expect("delivery phase must persist")
        );

        let captured = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route("/{*path}", post(fake_telegram_edit))
            .with_state(Arc::clone(&captured));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fake Telegram listener must bind");
        let address = listener.local_addr().expect("mock listener address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let bot = Bot::new("123456:TEST_TOKEN")
            .set_api_url(format!("http://{address}/").parse().expect("mock URL"))
            .parse_mode(ParseMode::Html);
        let port = FaultInjectingSendPort::new([
            DeliveryEffect::Confirmed(sent_message()),
            DeliveryEffect::Unknown(delivery_failure(DeliveryErrorKind::Transport)),
        ]);

        let delivery = send_rendered_transcript(
            &bot,
            &port,
            ChatId(media.chat_id),
            MessageId(media.message_id),
            MessageId(27),
            &RenderedTranscript::MessageAndFile {
                html: "confirmed preview".to_owned(),
                filename: "transcript.txt".to_owned(),
                body: "complete transcript".to_owned(),
            },
        )
        .await;
        server.abort();
        assert!(matches!(delivery, Err(VoiceDeliveryFailure::Unknown)));
        assert_eq!(*port.calls.lock().unwrap(), ["html", "document"]);
        assert!(captured.lock().unwrap().is_some());
        assert_eq!(
            mark_voice_job_delivery_unknown(&pool, &job, "ambiguous full transcript send")
                .await
                .expect("partial delivery state must persist"),
            VoiceTransition::DeliveryUnknown
        );
        pool.close().await;

        // A fresh pool models process restart and must not reclaim or duplicate
        // either the confirmed preview or the ambiguous full transcript send.
        let restarted_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("restarted test pool must connect");
        assert!(
            claim_voice_job(&restarted_pool, job_id)
                .await
                .expect("delivery_unknown job must remain queryable")
                .is_none()
        );
        assert_eq!(*port.calls.lock().unwrap(), ["html", "document"]);
        restarted_pool.close().await;
    }

    #[tokio::test]
    async fn confirmed_rich_payload_rejection_uses_one_html_fallback() {
        let port = FaultInjectingSendPort::new([
            DeliveryEffect::Rejected(delivery_failure(DeliveryErrorKind::PayloadRejected)),
            DeliveryEffect::Confirmed(sent_message()),
        ]);
        let bot = Bot::new("123456:TEST_TOKEN").parse_mode(ParseMode::Html);
        let fallback = RenderedTranscript::Message {
            html: "bounded fallback".to_owned(),
        };

        let result = send_rich_transcript_with_fallback(
            &bot,
            &port,
            ChatId(-100),
            MessageId(70),
            MessageId(71),
            "<p>full</p>",
            &fallback,
        )
        .await
        .unwrap();

        assert_eq!(result.html, "bounded fallback");
        assert_eq!(*port.calls.lock().unwrap(), ["rich", "html"]);
    }

    #[tokio::test]
    async fn oversized_rich_transcript_is_rejected_locally_before_port_send() {
        let port = FaultInjectingSendPort::new([DeliveryEffect::Confirmed(sent_message())]);
        let bot = Bot::new("123456:TEST_TOKEN").parse_mode(ParseMode::Html);
        let fallback = RenderedTranscript::Message {
            html: "bounded fallback".to_owned(),
        };

        let result = send_rich_transcript_with_fallback(
            &bot,
            &port,
            ChatId(-100),
            MessageId(70),
            MessageId(71),
            &"x".repeat(crate::telegram::render::TELEGRAM_RICH_TEXT_LIMIT + 1),
            &fallback,
        )
        .await
        .unwrap();

        assert_eq!(result.html, "bounded fallback");
        assert_eq!(*port.calls.lock().unwrap(), ["html"]);
    }

    #[test]
    fn common_no_speech_artifacts_are_not_speech() {
        assert!(!transcript_has_speech(&transcript("[музыка]", Vec::new())));
        assert!(!transcript_has_speech(&transcript(
            "Продолжение следует...",
            Vec::new()
        )));
        assert!(!transcript_has_speech(&transcript(
            "Субтитры сделал DimaTorzok",
            Vec::new()
        )));
    }

    #[test]
    fn real_short_words_are_speech() {
        assert!(transcript_has_speech(&transcript("да", Vec::new())));
        assert!(transcript_has_speech(&transcript("", vec!["нет"])))
    }

    #[test]
    fn one_word_artifacts_do_not_hide_real_speech() {
        assert!(transcript_has_speech(&transcript(
            "Музыка сегодня громкая",
            Vec::new()
        )));
    }

    async fn fake_telegram_edit(
        State(captured): State<CapturedTelegramRequest>,
        body: Bytes,
    ) -> axum::Json<serde_json::Value> {
        *captured.lock().expect("captured request lock") =
            Some(String::from_utf8_lossy(&body).into_owned());
        axum::Json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 27,
                "date": 1,
                "chat": {"id": -1001, "type": "supergroup", "title": "Test"},
                "text": "<b>recognized words</b>"
            }
        }))
    }

    #[tokio::test]
    async fn transcript_delivery_uses_the_fake_telegram_api() {
        let captured = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route("/{*path}", post(fake_telegram_edit))
            .with_state(Arc::clone(&captured));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fake Telegram listener must bind");
        let address = listener.local_addr().expect("fake listener address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let bot = Bot::new("123456:TEST_TOKEN")
            .set_api_url(format!("http://{address}/").parse().expect("mock URL"))
            .parse_mode(teloxide::types::ParseMode::Html);

        let delivered = send_rendered_transcript(
            &bot,
            &FaultInjectingSendPort::new(std::iter::empty::<DeliveryEffect<Message>>()),
            ChatId(-1001),
            MessageId(11),
            MessageId(27),
            &RenderedTranscript::Message {
                html: "<b>recognized words</b>".to_owned(),
            },
        )
        .await
        .expect("fake Telegram API should accept the transcript edit");
        server.abort();

        assert_eq!(delivered.html, "<b>recognized words</b>");
        let payload = captured
            .lock()
            .expect("captured request lock")
            .take()
            .expect("transcript delivery should call Telegram");
        assert!(payload.contains("chat_id"), "Telegram request: {payload}");
        assert!(payload.contains("-1001"), "Telegram request: {payload}");
        assert!(
            payload.contains("message_id"),
            "Telegram request: {payload}"
        );
        assert!(payload.contains("27"), "Telegram request: {payload}");
        assert!(
            payload.contains("<b>recognized words</b>"),
            "Telegram request: {payload}"
        );
    }
}
