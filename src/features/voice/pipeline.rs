use teloxide::prelude::*;
use teloxide::types::{InputFile, MessageId, ReplyParameters};

use crate::db::telegram::save_telegram_message;
use crate::features::voice::asr::transcribe_audio;
use crate::features::voice::cleanup::cleanup_transcript;
use crate::features::voice::download::{download_voice_file, validate_media};
use crate::features::voice::render::{RenderedTranscript, render_transcript};
use crate::features::voice::repo::{
    create_voice_job, mark_voice_job_failed, mark_voice_job_status, save_asr_result,
    save_voice_result,
};
use crate::features::voice::types::{AsrTranscript, VoiceMedia};
use crate::state::AppState;
use crate::telegram::render::send_html_reply;

const NO_SPEECH_MESSAGE: &str =
    "В голосовом не нашёл распознаваемой речи — не буду додумывать текст.";

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

    if (!msg.chat.is_private() && msg.chat.id.0 != state.config.discussion_chat_id)
        || msg.from.as_ref().is_some_and(|user| user.is_bot)
        || msg
            .text()
            .is_some_and(|text| text.trim_start().starts_with('/'))
        || msg.is_automatic_forward()
    {
        return Ok(false);
    }

    let Some(media) = VoiceMedia::from_message(msg) else {
        return Ok(false);
    };

    save_telegram_message(&state.pool, msg).await?;

    let Some(job_id) = create_voice_job(&state.pool, &media).await? else {
        tracing::debug!(
            chat_id = media.chat_id,
            message_id = media.message_id,
            "voice transcription job already exists"
        );
        return Ok(true);
    };

    if let Err(skip) = validate_media(&media, &state.config) {
        mark_voice_job_failed(&state.pool, job_id, &skip.user_message()).await?;
        send_html_reply(
            bot,
            msg.chat.id,
            msg.id,
            crate::telegram::render::escape_html(&skip.user_message()),
        )
        .await?;
        return Ok(true);
    }

    if let Err(err) = process_voice_job(bot, msg, state, job_id, &media).await {
        mark_voice_job_failed(&state.pool, job_id, &err.to_string()).await?;
        return Err(err);
    }

    Ok(true)
}

async fn process_voice_job(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    job_id: i64,
    media: &VoiceMedia,
) -> anyhow::Result<()> {
    mark_voice_job_status(&state.pool, job_id, "downloading").await?;
    let downloaded = download_voice_file(bot, media).await?;
    tracing::info!(
        job_id,
        size = downloaded.size,
        "downloaded voice file for transcription"
    );

    mark_voice_job_status(&state.pool, job_id, "transcribing").await?;
    let transcript = transcribe_audio(
        &state.config,
        &downloaded.path,
        &downloaded.filename,
        downloaded.mime_type.as_deref(),
    )
    .await?;
    save_asr_result(&state.pool, job_id, &transcript).await?;
    if !transcript_has_speech(&transcript) {
        mark_voice_job_failed(&state.pool, job_id, NO_SPEECH_MESSAGE).await?;
        send_html_reply(
            bot,
            msg.chat.id,
            msg.id,
            crate::telegram::render::escape_html(NO_SPEECH_MESSAGE),
        )
        .await?;
        return Ok(());
    }

    mark_voice_job_status(&state.pool, job_id, "cleaning").await?;
    let clean = cleanup_transcript(&state.config, &transcript).await?;
    let rendered = render_transcript(&clean, &state.config);
    let file_id = send_rendered_transcript(bot, msg, &rendered).await?;
    save_voice_result(
        &state.pool,
        job_id,
        &clean,
        rendered.html(),
        file_id.as_deref(),
    )
    .await?;

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

async fn send_rendered_transcript(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    rendered: &RenderedTranscript,
) -> anyhow::Result<Option<String>> {
    match rendered {
        RenderedTranscript::Message { html } => {
            send_html_reply(bot, msg.chat.id, msg.id, html).await?;
            Ok(None)
        }
        RenderedTranscript::MessageAndFile {
            html,
            filename,
            body,
        } => {
            send_html_reply(bot, msg.chat.id, msg.id, html).await?;
            let sent = bot
                .send_document(
                    msg.chat.id,
                    InputFile::memory(body.clone().into_bytes()).file_name(filename.clone()),
                )
                .reply_parameters(
                    ReplyParameters::new(MessageId(msg.id.0)).allow_sending_without_reply(),
                )
                .await?;
            Ok(sent.document().map(|document| document.file.id.clone()))
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
