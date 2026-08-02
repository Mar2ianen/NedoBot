use teloxide::prelude::*;
use teloxide::types::{InputFile, InputRichMessage, MessageId, ReplyParameters};

use crate::db::telegram::save_telegram_message;
use crate::features::voice::asr::transcribe_audio;
use crate::features::voice::cleanup::cleanup_transcript;
use crate::features::voice::download::{download_media_file, validate_media};
use crate::features::voice::render::{RenderedTranscript, render_transcript};
use crate::features::voice::repo::{
    VoiceJob, claim_next_voice_job, claim_voice_job, create_voice_job, mark_voice_job_failed,
    mark_voice_job_phase, mark_voice_job_retry_or_failed, mark_voice_job_skipped, save_asr_result,
    save_progress_message, save_voice_result,
};
use crate::features::voice::types::{AsrTranscript, VoiceMedia};
use crate::state::AppState;
use crate::telegram::render::{send_html_reply, send_rich_message_reply};

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
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
) -> anyhow::Result<bool> {
    if !state.config.voice_transcription_enabled || !state.config.voice_auto_transcribe {
        return Ok(false);
    }

    if !voice_chat_is_supported(msg, state)
        || msg.from.as_ref().is_some_and(|user| user.is_bot)
        || msg
            .text()
            .is_some_and(|text| text.trim_start().starts_with('/'))
        || msg.is_automatic_forward()
    {
        return Ok(false);
    }

    transcribe_media_message(bot, msg, state).await
}

pub async fn transcribe_reply(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    command_message: &Message,
    state: &AppState,
) -> anyhow::Result<()> {
    if !state.config.voice_transcription_enabled {
        return send_voice_command_message(
            bot,
            command_message,
            "Расшифровка голосовых сейчас отключена.",
        )
        .await;
    }
    if !voice_chat_is_supported(command_message, state) {
        return send_voice_command_message(
            bot,
            command_message,
            "Эта команда доступна только в личном чате или основном чате.",
        )
        .await;
    }

    let Some(reply) = command_message.reply_to_message() else {
        return send_voice_command_message(
            bot,
            command_message,
            "Ответьте командой /transcribe на voice, audio или кружок.",
        )
        .await;
    };
    if VoiceMedia::from_message(reply).is_none() {
        return send_voice_command_message(
            bot,
            command_message,
            "Нужен reply на voice, audio или кружок.",
        )
        .await;
    }

    transcribe_media_message(bot, reply, state).await?;
    Ok(())
}

fn voice_chat_is_supported(msg: &Message, state: &AppState) -> bool {
    msg.chat.is_private() || msg.chat.id.0 == state.config.discussion_chat_id
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
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
) -> anyhow::Result<bool> {
    let Some(media) = VoiceMedia::from_message(msg) else {
        return Ok(false);
    };

    save_telegram_message(&state.pool, msg, &state.config).await?;

    let Some(job_id) = create_voice_job(&state.pool, &media).await? else {
        tracing::debug!(
            chat_id = media.chat_id,
            message_id = media.message_id,
            "voice transcription job is already processing or terminal"
        );
        return Ok(true);
    };

    let Some(job) = claim_voice_job(&state.pool, job_id).await? else {
        tracing::debug!(
            job_id,
            "voice transcription job was claimed by another worker"
        );
        return Ok(true);
    };
    process_claimed_voice_job(bot, state, job, media).await?;
    Ok(true)
}

pub async fn process_next_voice_job(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    state: &AppState,
) -> anyhow::Result<bool> {
    let Some(job) = claim_next_voice_job(&state.pool).await? else {
        return Ok(false);
    };
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
        mark_voice_job_failed(&state.pool, &job, "validation_failed", &skip.user_message()).await?;
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
        tracing::warn!(
            job_id = job.id,
            error_kind = failure.error_kind(),
            "voice transcription failed"
        );
        mark_voice_job_retry_or_failed(
            &state.pool,
            &job,
            failure.error_kind(),
            failure
                .user_message()
                .unwrap_or("voice transcription failed"),
        )
        .await?;
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
    Delivery,
    LeaseLost,
}

impl VoiceProcessingFailure {
    fn error_kind(self) -> &'static str {
        match self {
            Self::Download => "download_failed",
            Self::Asr => "asr_failed",
            Self::Cleanup => "cleanup_failed",
            Self::Delivery => "delivery_failed",
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
            Self::Delivery => None,
            Self::LeaseLost => None,
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
    mark_voice_job_phase(&state.pool, job, "downloading")
        .await
        .map_err(|_| VoiceProcessingFailure::Download)?
        .then_some(())
        .ok_or(VoiceProcessingFailure::LeaseLost)?;
    let transcript = {
        let downloaded = download_media_file(bot, media)
            .await
            .map_err(|_| VoiceProcessingFailure::Download)?;
        tracing::info!(
            job_id = job.id,
            size = downloaded.size,
            media_kind = media.kind.as_str(),
            "downloaded media file for transcription"
        );

        mark_voice_job_phase(&state.pool, job, "transcribing")
            .await
            .map_err(|_| VoiceProcessingFailure::Asr)?
            .then_some(())
            .ok_or(VoiceProcessingFailure::LeaseLost)?;
        transcribe_audio(
            &state.config,
            &downloaded.path,
            &downloaded.filename,
            downloaded.mime_type.as_deref(),
        )
        .await
        .map_err(|_| VoiceProcessingFailure::Asr)?
    };
    save_asr_result(&state.pool, job, &transcript)
        .await
        .map_err(|_| VoiceProcessingFailure::Asr)?
        .then_some(())
        .ok_or(VoiceProcessingFailure::LeaseLost)?;
    if !transcript_has_speech(&transcript) {
        mark_voice_job_skipped(&state.pool, job, NO_SPEECH_MESSAGE)
            .await
            .map_err(|_| VoiceProcessingFailure::Delivery)?
            .then_some(())
            .ok_or(VoiceProcessingFailure::LeaseLost)?;
        edit_transcription_message(
            bot,
            ChatId(media.chat_id),
            progress_message_id,
            NO_SPEECH_MESSAGE,
        )
        .await
        .map_err(|_| VoiceProcessingFailure::Delivery)?;
        return Ok(());
    }

    mark_voice_job_phase(&state.pool, job, "cleaning")
        .await
        .map_err(|_| VoiceProcessingFailure::Cleanup)?
        .then_some(())
        .ok_or(VoiceProcessingFailure::LeaseLost)?;
    let cleanup = cleanup_transcript(&state.config, &transcript)
        .await
        .map_err(|_| VoiceProcessingFailure::Cleanup)?;
    let rendered = render_transcript(&cleanup.transcript, &state.config);
    let sent = send_rendered_transcript(
        bot,
        ChatId(media.chat_id),
        MessageId(media.message_id),
        progress_message_id,
        &rendered,
    )
    .await
    .map_err(|_| VoiceProcessingFailure::Delivery)?;
    save_voice_result(
        &state.pool,
        job,
        &cleanup,
        &sent.html,
        sent.file_id.as_deref(),
    )
    .await
    .map_err(|_| VoiceProcessingFailure::Delivery)?
    .then_some(())
    .ok_or(VoiceProcessingFailure::LeaseLost)?;

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
    chat_id: ChatId,
    source_message_id: MessageId,
    progress_message_id: MessageId,
    rendered: &RenderedTranscript,
) -> anyhow::Result<SentRenderedTranscript> {
    match rendered {
        RenderedTranscript::Message { html } => {
            edit_transcription_message(bot, chat_id, progress_message_id, html).await?;
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
            .await?;
            let rich = InputRichMessage::html(html.clone()).skip_entity_detection(true);
            match send_rich_message_reply(bot, chat_id, source_message_id, rich).await {
                Ok(_) => Ok(SentRenderedTranscript {
                    html: html.clone(),
                    file_id: None,
                }),
                Err(_) => send_regular_transcript(bot, chat_id, source_message_id, fallback).await,
            }
        }
        RenderedTranscript::MessageAndFile { .. } => {
            edit_transcription_message(
                bot,
                chat_id,
                progress_message_id,
                "Расшифровка готова. Полный текст — следующим сообщением.",
            )
            .await?;
            send_regular_transcript(bot, chat_id, source_message_id, rendered).await
        }
    }
}

async fn send_regular_transcript(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    source_message_id: MessageId,
    rendered: &RenderedTranscript,
) -> anyhow::Result<SentRenderedTranscript> {
    match rendered {
        RenderedTranscript::Message { html } => {
            send_html_reply(bot, chat_id, source_message_id, html).await?;
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
            send_html_reply(bot, chat_id, source_message_id, html).await?;
            let sent = bot
                .send_document(
                    chat_id,
                    InputFile::memory(body.clone().into_bytes()).file_name(filename.clone()),
                )
                .reply_parameters(
                    ReplyParameters::new(source_message_id).allow_sending_without_reply(),
                )
                .await?;
            Ok(SentRenderedTranscript {
                html: html.clone(),
                file_id: sent.document().map(|document| document.file.id.to_string()),
            })
        }
        RenderedTranscript::RichMessage { .. } => {
            anyhow::bail!("rich voice transcript fallback must be a regular message")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::voice::types::AsrSegment;

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
}
