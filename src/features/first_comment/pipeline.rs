use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use teloxide::{net::Download, prelude::*, types::MessageId};

use crate::config::Config;
use crate::db::telegram::save_telegram_message;
use crate::features::first_comment::candidate::comment_candidate;
use crate::features::first_comment::clean::{clean_post_for_llm, should_generate_comment};
use crate::features::first_comment::prompt::build_llm_prompt;
use crate::features::first_comment::quality::validate_comment_output;
use crate::features::first_comment::render::build_comment_html;
use crate::features::first_comment::repo::{
    LlmGenerationInsert, create_post_comment_job, insert_llm_generation, load_recent_bot_comments,
    mark_post_comment_sent,
};
use crate::features::memory::service::{load_relevant_memory_notes, remember_post};
use crate::llm::service::generate_text_checked;
use crate::state::AppState;
use crate::telegram::render::{send_html, send_html_reply};

pub async fn maybe_comment_post(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
) -> anyhow::Result<()> {
    let pool = &state.pool;
    let config = &state.config;

    save_telegram_message(pool, msg).await?;

    // The bot should never react to random chat messages. A valid target is only
    // Telegram's automatic channel post copy in the linked discussion chat.
    let Some(candidate) = comment_candidate(msg, config) else {
        return Ok(());
    };

    // Editorial posts carry the VK/MAX footer. Ads usually do not, so the marker
    // doubles as a cheap allowlist and keeps promotional posts out of the chat CTA.
    if !should_generate_comment(candidate.post_text, config) {
        tracing::info!(
            discussion_message_id = msg.id.0,
            "skip post without signature marker"
        );
        return Ok(());
    }

    let clean_post = clean_post_for_llm(candidate.post_text, config);
    let job_id = create_post_comment_job(
        pool,
        config.discussion_chat_id,
        msg.id.0,
        candidate.source_channel_id,
        candidate.source_message_id.0,
        &clean_post,
    )
    .await?;

    let Some(job_id) = job_id else {
        tracing::info!(
            discussion_message_id = msg.id.0,
            "comment job already exists, skip"
        );
        return Ok(());
    };

    let image_base64 = match download_largest_photo_base64(bot, msg).await {
        Ok(image) => image,
        Err(err) => {
            tracing::warn!(%err, "failed to download post image, continue text-only");
            None
        }
    };
    let chat_member_count = get_chat_member_count(bot, config).await;
    let memory_notes = load_relevant_memory_notes(pool, &clean_post).await?;
    let recent_comments = load_recent_bot_comments(pool).await?;
    let prompt = build_llm_prompt(
        &clean_post,
        chat_member_count,
        &memory_notes,
        &recent_comments,
        None,
    );
    let generation = generate_text_checked(
        config,
        &prompt,
        image_base64.as_deref(),
        config.llm_temperature,
        config.llm_max_tokens,
        Some(validate_comment_output),
    )
    .await?;
    let final_html = build_comment_html(&generation.content, config);
    ensure_comment_html(&final_html, &generation.content)?;

    let sent = send_html_reply(bot, msg.chat.id, msg.id, final_html.clone()).await?;

    mark_post_comment_sent(pool, job_id, sent.id.0).await?;
    insert_llm_generation(
        pool,
        LlmGenerationInsert {
            job_id,
            provider: &generation.provider,
            model: &generation.model,
            prompt: &prompt,
            image_used: generation.image_used,
            response: &generation.content,
            final_html: &final_html,
        },
    )
    .await?;

    if let Some(owner_id) = owner_preview_chat(config) {
        send_owner_preview(bot, owner_id, &final_html, candidate.source_message_id).await;
    }

    if let Err(err) = remember_post(
        pool,
        config,
        candidate.source_channel_id,
        candidate.source_message_id.0,
        &clean_post,
    )
    .await
    {
        tracing::warn!(%err, "failed to save post memory note");
    }

    Ok(())
}

fn ensure_comment_html(final_html: &str, raw_response: &str) -> anyhow::Result<()> {
    if final_html.trim().is_empty() {
        anyhow::bail!(
            "empty rendered comment from LLM response: {}",
            raw_response.chars().take(120).collect::<String>()
        );
    }

    Ok(())
}

fn owner_preview_chat(config: &Config) -> Option<i64> {
    config
        .send_owner_preview
        .then_some(config.owner_telegram_id)?
}

async fn send_owner_preview(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    owner_id: i64,
    final_html: &str,
    source_message_id: MessageId,
) {
    let preview = format!(
        "Комментарий отправлен:\n\n{}\n\n<code>source_message_id={}</code>",
        final_html, source_message_id.0
    );

    if let Err(err) = send_html(bot, ChatId(owner_id), preview).await {
        tracing::warn!(%err, "failed to send owner preview");
    }
}

async fn download_largest_photo_base64(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
) -> anyhow::Result<Option<String>> {
    let Some(photo) = msg
        .photo()
        .and_then(|photos| photos.iter().max_by_key(|photo| photo.width * photo.height))
    else {
        return Ok(None);
    };

    let file = bot.get_file(photo.file.id.clone()).await?;
    let mut bytes = Vec::new();
    bot.download_file(&file.path, &mut bytes).await?;

    Ok(Some(BASE64.encode(bytes)))
}

async fn get_chat_member_count(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    config: &Config,
) -> Option<u32> {
    match bot
        .get_chat_member_count(ChatId(config.discussion_chat_id))
        .await
    {
        Ok(count) => Some(count),
        Err(err) => {
            tracing::warn!(%err, "failed to get chat member count");
            None
        }
    }
}
