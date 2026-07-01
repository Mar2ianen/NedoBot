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
use crate::features::voice::types::VoiceMedia;
use crate::state::AppState;
use crate::telegram::render::send_html_reply;

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
