use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use teloxide::{net::Download, prelude::*, types::MessageId};
use tokio::io::AsyncWrite;

use crate::config::Config;
use crate::db::telegram::save_telegram_message;
use crate::features::first_comment::candidate::comment_candidate;
use crate::features::first_comment::clean::{clean_post_for_llm, should_generate_comment};
use crate::features::first_comment::draft::{
    first_comment_output_schema, parse_first_comment_draft,
    validate_first_comment_draft_with_search_and_policy,
};
use crate::features::first_comment::prompt::{CommentDirectives, build_llm_prompt_parts};
use crate::features::first_comment::render::build_comment_html_with_sources;
use crate::features::first_comment::repo::{
    LlmGenerationInsert, create_post_comment_job, insert_llm_generation, load_recent_bot_comments,
    load_topic_bot_comments, mark_post_comment_sent,
};
use crate::features::memory::service::{load_relevant_memory_notes, remember_post};
use crate::features::search::repo::insert_search_run;
use crate::features::search::service::run_search;
use crate::features::search::types::SearchContext;
use crate::llm::service::generate_text_checked_with_system_and_schema;
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

    let image_base64 = match download_largest_photo_base64(bot, msg, config).await {
        Ok(image) => image,
        Err(err) => {
            tracing::warn!(%err, "failed to download post image, continue text-only");
            None
        }
    };
    let chat_member_count = get_chat_member_count(bot, config).await;
    let memory_notes = load_relevant_memory_notes(pool, &clean_post).await?;
    let recent_comments = load_recent_bot_comments(pool).await?;
    let topic_comments = load_topic_bot_comments(pool, &clean_post).await?;
    let search_context = run_search(config, &clean_post).await;
    if let Err(err) = insert_search_run(pool, job_id, &search_context).await {
        tracing::warn!(%err, "failed to save search run");
    }
    let directives =
        CommentDirectives::for_post(candidate.source_message_id.0, Some(&search_context));
    let prompt = build_llm_prompt_parts(
        &clean_post,
        chat_member_count,
        &memory_notes,
        &recent_comments,
        &topic_comments,
        config.search_enabled.then_some(&search_context),
        directives,
    );
    let validation_results = search_context.results.clone();
    let source_link_available = directives.source_link_available();
    let source_policy = config.clone();
    let validator = move |value: &str| {
        validate_first_comment_draft_with_search_and_policy(
            value,
            &validation_results,
            source_link_available,
            &source_policy,
        )
    };
    let generation = generate_text_checked_with_system_and_schema(
        config,
        &prompt.system,
        &prompt.user,
        image_base64.as_deref(),
        config.llm_temperature,
        config.llm_max_tokens,
        Some(&validator),
        "first_comment_draft",
        first_comment_output_schema(),
    )
    .await?;
    let draft = parse_first_comment_draft(&generation.content)?;
    let used_search_result_id = draft.used_search_result_id.map(|id| id as i32);
    let prompt_for_log = prompt.compact_for_log();
    let attempts = serde_json::to_value(&generation.attempts)?;
    let final_html =
        build_comment_html_with_sources(&draft.comment, config, &search_context.results);
    ensure_comment_html(&final_html, &draft.comment)?;

    let sent = send_html_reply(bot, msg.chat.id, msg.id, final_html.clone()).await?;

    mark_post_comment_sent(pool, job_id, sent.id.0).await?;
    insert_llm_generation(
        pool,
        LlmGenerationInsert {
            job_id,
            provider: &generation.provider,
            model: &generation.model,
            prompt: &prompt_for_log,
            image_used: generation.image_used,
            response: &draft.comment,
            final_html: &final_html,
            attempts: &attempts,
            used_search_result_id,
        },
    )
    .await?;

    if let Some(owner_id) = owner_preview_chat(config) {
        send_owner_preview(
            bot,
            owner_id,
            &final_html,
            candidate.source_message_id,
            &search_context,
            used_search_result_id,
        )
        .await;
    }

    if let Err(err) = remember_post(
        pool,
        config,
        candidate.source_channel_id,
        candidate.source_message_id.0,
        &clean_post,
        &draft.comment,
        draft
            .used_search_result_id
            .and_then(|id| search_context.results.get(id.saturating_sub(1))),
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

fn render_search_summary(search_context: &SearchContext) -> String {
    match search_context.skipped_reason.as_deref() {
        Some(reason) => format!("search=skipped({reason}), {}ms", search_context.latency_ms),
        None => format!(
            "search={} queries, {} results, {}ms",
            search_context.queries.len(),
            search_context.results.len(),
            search_context.latency_ms
        ),
    }
}

async fn send_owner_preview(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    owner_id: i64,
    final_html: &str,
    source_message_id: MessageId,
    search_context: &SearchContext,
    used_search_result_id: Option<i32>,
) {
    let preview = format!(
        "Комментарий отправлен:\n\n{}\n\n<code>source_message_id={}</code>\n<code>used_search_result_id={}</code>\n<code>{}</code>",
        final_html,
        source_message_id.0,
        used_search_result_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "null".to_string()),
        render_search_summary(search_context)
    );

    if let Err(err) = send_html(bot, ChatId(owner_id), preview).await {
        tracing::warn!(%err, "failed to send owner preview");
    }
}

pub(crate) async fn download_largest_photo_base64(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    config: &Config,
) -> anyhow::Result<Option<String>> {
    let Some(photo) = msg
        .photo()
        .and_then(|photos| photos.iter().max_by_key(|photo| photo.width * photo.height))
    else {
        return Ok(None);
    };

    let file = bot.get_file(photo.file.id.clone()).await?;
    let max_bytes = u64::from(config.first_comment_max_image_mb) * 1024 * 1024;
    if u64::from(file.size) > max_bytes {
        anyhow::bail!(
            "post image exceeds configured limit of {} MB",
            config.first_comment_max_image_mb
        );
    }
    let max_bytes =
        usize::try_from(max_bytes).map_err(|_| anyhow::anyhow!("image limit is too large"))?;
    let mut bytes = LimitedBytesWriter::new(max_bytes);
    bot.download_file(&file.path, &mut bytes).await?;

    Ok(Some(BASE64.encode(bytes.into_inner())))
}

struct LimitedBytesWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl LimitedBytesWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(1024 * 1024)),
            limit,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl AsyncWrite for LimitedBytesWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if buf.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "image exceeds configured download limit",
            )));
        }

        self.bytes.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn limited_bytes_writer_rejects_overflow() {
        let mut writer = LimitedBytesWriter::new(4);
        writer.write_all(b"1234").await.unwrap();
        let err = writer.write_all(b"5").await.unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::WriteZero);
        assert_eq!(writer.into_inner(), b"1234");
    }
}
