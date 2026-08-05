use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use teloxide::{
    net::Download,
    prelude::*,
    types::{FileId, MessageId},
};
use tokio::io::AsyncWrite;

use crate::config::Config;
use crate::db::telegram::save_telegram_message;
use crate::features::first_comment::candidate::comment_candidate;
use crate::features::first_comment::clean::{clean_post_for_llm, should_generate_comment};
use crate::features::first_comment::draft::{
    FirstCommentDraft, first_comment_output_schema, parse_first_comment_draft,
    validate_first_comment_draft_with_search_policy_and_chat,
};
use crate::features::first_comment::prompt::{
    ChatEvidence, CommentDirectives, FirstCommentPromptInput,
    build_llm_prompt_parts_with_chat_evidence,
};
use crate::features::first_comment::repo::{
    CommentErrorKind, CreatePostCommentJobParams, FinalizePostCommentSent, LlmGenerationInsert,
    PostCommentJob, SendFailure, begin_post_comment_delivery, claim_next_post_comment_job,
    classify_send_error, create_post_comment_job, finalize_post_comment_sent,
    load_recent_bot_comments, mark_operator_retry_post_comment_terminal_failed,
    mark_post_comment_delivery_unknown, mark_post_comment_pre_send_failed,
    mark_post_comment_send_rejected,
};
use crate::features::jobs::claim::CasResult;
use crate::features::memory::service::load_relevant_memory_notes;
use crate::features::search::repo::{
    insert_search_run, save_chat_evidence_outcome, save_chat_retrieval_candidates,
    save_expanded_chat_contexts,
};
use crate::features::search::service::run_search;
use crate::features::search::types::SearchContext;
use crate::llm::service::{GenerateTextOptions, generate_text_checked};
use crate::state::AppState;
use crate::telegram::render::{send_html, send_html_reply};

pub async fn maybe_comment_post(msg: &Message, state: &AppState) -> anyhow::Result<()> {
    let pool = &state.pool;
    let config = &state.config;

    save_telegram_message(pool, msg, config).await?;

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
    let image = msg
        .photo()
        .and_then(|photos| photos.iter().max_by_key(|photo| photo.width * photo.height));
    let job_id = create_post_comment_job(
        pool,
        CreatePostCommentJobParams {
            discussion_chat_id: config.discussion_chat_id,
            discussion_message_id: msg.id.0,
            source_channel_id: candidate.source_channel_id,
            source_message_id: candidate.source_message_id.0,
            cleaned_post_text: &clean_post,
            image_file_id: image.map(|photo| photo.file.id.0.as_str()),
            image_file_unique_id: image.map(|photo| photo.file.unique_id.0.as_str()),
        },
    )
    .await?;

    if let Some(job_id) = job_id {
        tracing::info!(
            job_id,
            discussion_message_id = msg.id.0,
            "comment job enqueued"
        );
    } else {
        tracing::info!(
            discussion_message_id = msg.id.0,
            "comment job already exists, skip"
        );
    }

    Ok(())
}

enum JobOutcome {
    Prepared(Box<CompletedComment>),
    Completed,
    Failed(CommentErrorKind),
    LeaseLost,
}

struct CompletedComment {
    generation: crate::llm::types::GeneratedText,
    draft: FirstCommentDraft,
    prompt_for_log: String,
    final_html: String,
    search_context: SearchContext,
    used_search_result_id: Option<i32>,
}

pub async fn process_next_post_comment_job(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    state: &AppState,
) -> anyhow::Result<bool> {
    let Some(job) = claim_next_post_comment_job(&state.pool).await? else {
        return Ok(false);
    };

    process_claimed_post_comment_job(bot, state, &job).await?;

    Ok(true)
}

pub async fn process_claimed_post_comment_job(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    state: &AppState,
    job: &PostCommentJob,
) -> anyhow::Result<()> {
    let outcome = match process_post_comment_job(bot, state, job).await {
        Ok(outcome) => outcome,
        Err(error_kind) => JobOutcome::Failed(error_kind),
    };
    match outcome {
        JobOutcome::Prepared(completed) => {
            match deliver_prepared_post_comment(bot, state, job, completed).await? {
                JobOutcome::LeaseLost => tracing::info!(
                    job_id = job.id,
                    "post comment worker lost its current delivery attempt"
                ),
                JobOutcome::Failed(error_kind) => tracing::warn!(
                    job_id = job.id,
                    attempts = job.attempts,
                    ?error_kind,
                    "post comment delivery was rejected"
                ),
                JobOutcome::Completed => {}
                JobOutcome::Prepared(_) => unreachable!("delivery cannot prepare another comment"),
            }
        }
        JobOutcome::Completed => {}
        JobOutcome::Failed(error_kind) => {
            let result = if job.operator_retry_only {
                mark_operator_retry_post_comment_terminal_failed(&state.pool, job, error_kind)
                    .await?
            } else {
                mark_post_comment_pre_send_failed(&state.pool, job, error_kind).await?
            };
            if result == CasResult::Applied {
                tracing::warn!(
                    job_id = job.id,
                    attempts = job.attempts,
                    ?error_kind,
                    "post comment job failed before Telegram delivery"
                );
            } else {
                tracing::info!(job_id = job.id, "post comment worker lost processing lease");
            }
        }
        JobOutcome::LeaseLost => {
            tracing::info!(job_id = job.id, "post comment worker lost processing lease");
        }
    }

    Ok(())
}

async fn process_post_comment_job(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    state: &AppState,
    job: &PostCommentJob,
) -> Result<JobOutcome, CommentErrorKind> {
    let pool = &state.pool;
    let config = &state.config;
    let image_base64 = download_photo_base64(bot, job.image_file_id.as_deref(), config)
        .await
        .map_err(|_| CommentErrorKind::ImageUnavailable)?;
    let chat_member_count = get_chat_member_count(bot, config).await;
    let memory_notes = load_relevant_memory_notes(pool, config, &job.cleaned_post_text)
        .await
        .map_err(|_| CommentErrorKind::Transient)?;
    let recent_comments = load_recent_bot_comments(pool)
        .await
        .map_err(|_| CommentErrorKind::Transient)?;
    let topic_comments = Vec::new();
    let search_context = run_search(config, &job.cleaned_post_text, &memory_notes).await;
    if let Err(err) = insert_search_run(pool, job.id, &search_context).await {
        tracing::warn!(%err, "failed to save search run");
    }
    let mut chat_candidates = Vec::new();
    let mut expanded_chat_contexts = Vec::new();
    if let Some(plan) = search_context.plan.as_ref() {
        match crate::features::chat_retrieval::run_shadow_retrieval(
            pool,
            config,
            config.discussion_chat_id,
            plan,
        )
        .await
        {
            Ok(candidates) => {
                chat_candidates = candidates.clone();
                if let Err(err) = save_chat_retrieval_candidates(pool, job.id, &candidates).await {
                    tracing::warn!(%err, "failed to save chat retrieval shadow run");
                }
                match crate::features::chat_retrieval::expand_shadow_contexts(
                    pool,
                    config.discussion_chat_id,
                    &candidates,
                )
                .await
                {
                    Ok(contexts) => {
                        if let Err(err) = save_expanded_chat_contexts(pool, job.id, &contexts).await
                        {
                            tracing::warn!(%err, "failed to save expanded chat contexts");
                        }
                        expanded_chat_contexts = contexts;
                    }
                    Err(err) => tracing::warn!(%err, "failed to expand chat retrieval contexts"),
                }
            }
            Err(err) => tracing::warn!(%err, "chat retrieval shadow run failed"),
        }
    }
    let directives = CommentDirectives::for_post(job.source_message_id, Some(&search_context));
    let evidence_candidates = chat_candidates
        .iter()
        .filter(|candidate| {
            config.chat_retrieval_evidence_enabled
                && candidate.total_score >= config.chat_retrieval_evidence_min_score
        })
        .collect::<Vec<_>>();
    let chat_candidate_ids = evidence_candidates
        .iter()
        .map(|candidate| candidate.message_id)
        .collect::<Vec<_>>();
    let chat_targets = crate::features::first_comment::repo::load_chat_link_targets(
        pool,
        config.discussion_chat_id,
        &chat_candidate_ids,
    )
    .await
    .map_err(|_| CommentErrorKind::Transient)?;
    let chat_evidence = if config.chat_retrieval_evidence_enabled {
        evidence_candidates
            .iter()
            .filter_map(|candidate| {
                chat_targets
                    .iter()
                    .find(|target| target.message_id == candidate.message_id)
                    .map(|target| {
                        let context = expanded_chat_contexts
                            .iter()
                            .find(|context| context.anchor_message_id == candidate.message_id);
                        ChatEvidence {
                            candidate,
                            author_name: &target.author_name,
                            context,
                        }
                    })
            })
            .collect::<Vec<_>>()
    } else {
        Default::default()
    };
    let prompt = build_llm_prompt_parts_with_chat_evidence(FirstCommentPromptInput {
        post_text: &job.cleaned_post_text,
        chat_member_count,
        memory_notes: &memory_notes,
        recent_comments: &recent_comments,
        topic_comments: &topic_comments,
        search_context: config.search_enabled.then_some(&search_context),
        directives,
        chat_evidence: &chat_evidence,
    });
    let validation_results = search_context.results.clone();
    let source_link_available = directives.source_link_available();
    let source_policy = config.clone();
    let allowed_chat_message_ids = if config.chat_retrieval_evidence_enabled {
        chat_candidate_ids.clone()
    } else {
        Vec::new()
    };
    let validator = move |value: &str| {
        validate_first_comment_draft_with_search_policy_and_chat(
            value,
            &validation_results,
            source_link_available,
            &source_policy,
            &allowed_chat_message_ids,
        )
    };
    let generation = generate_text_checked(
        config,
        GenerateTextOptions {
            route: "first_comment",
            system_prompt: Some(&prompt.system),
            prompt: &prompt.user,
            image_base64: image_base64.as_deref(),
            temperature: config.llm_temperature,
            num_predict: config.llm_max_tokens,
            output_validator: Some(&validator),
            structured_output: Some(crate::llm::types::StructuredOutput {
                name: "first_comment_draft",
                schema: first_comment_output_schema(),
            }),
        },
    )
    .await
    .map_err(|error| CommentErrorKind::from_llm_error(&error))?;
    let draft = parse_first_comment_draft(&generation.content)
        .map_err(|_| CommentErrorKind::InvalidInput)?;
    if draft
        .used_chat_message_ids
        .iter()
        .any(|id| !chat_candidate_ids.contains(id))
    {
        return Err(CommentErrorKind::InvalidInput);
    }
    let chat_targets = chat_targets
        .into_iter()
        .filter(|target| {
            config.chat_retrieval_evidence_enabled
                && draft.used_chat_message_ids.contains(&target.message_id)
        })
        .collect::<Vec<_>>();
    let evidence_rejection_reason = if !config.chat_retrieval_evidence_enabled {
        Some("evidence_disabled")
    } else if evidence_candidates.is_empty() {
        Some("no_high_confidence_candidate")
    } else if draft.used_chat_message_ids.is_empty() {
        Some("model_declined_evidence")
    } else {
        None
    };
    if let Err(err) = save_chat_evidence_outcome(
        pool,
        job.id,
        &draft.used_chat_message_ids,
        evidence_rejection_reason,
    )
    .await
    {
        tracing::warn!(%err, "failed to save chat evidence outcome");
    }
    let used_search_result_id = draft.used_search_result_id.map(|id| id as i32);
    let prompt_for_log = prompt.compact_for_log();
    let final_html = crate::features::first_comment::render::build_comment_html_with_context(
        &draft.comment,
        config,
        &search_context.results,
        &chat_targets,
    );
    ensure_comment_html(&final_html, &draft.comment).map_err(|_| CommentErrorKind::InvalidInput)?;

    Ok(JobOutcome::Prepared(Box::new(CompletedComment {
        generation,
        draft,
        prompt_for_log,
        final_html,
        search_context,
        used_search_result_id,
    })))
}

async fn deliver_prepared_post_comment(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    state: &AppState,
    job: &PostCommentJob,
    completed: Box<CompletedComment>,
) -> anyhow::Result<JobOutcome> {
    if begin_post_comment_delivery(&state.pool, job).await? == CasResult::LeaseLost {
        return Ok(JobOutcome::LeaseLost);
    }

    let sent = match send_html_reply(
        bot,
        ChatId(job.discussion_chat_id),
        MessageId(job.discussion_message_id),
        completed.final_html.clone(),
    )
    .await
    {
        Ok(sent) => sent,
        Err(error) => return handle_post_comment_send_error(state, job, &error).await,
    };

    if let Err(err) = save_telegram_message(&state.pool, &sent, &state.config).await {
        tracing::warn!(%err, message_id = sent.id.0, "failed to save bot comment message");
    }

    finalize_completed_post_comment_job(bot, state, job, completed, sent.id.0).await
}

async fn handle_post_comment_send_error(
    state: &AppState,
    job: &PostCommentJob,
    error: &teloxide::RequestError,
) -> anyhow::Result<JobOutcome> {
    match classify_send_error(error) {
        SendFailure::Confirmed {
            error_kind,
            retry_after_seconds,
        } => {
            let result = if job.operator_retry_only {
                mark_operator_retry_post_comment_terminal_failed(&state.pool, job, error_kind)
                    .await?
            } else {
                mark_post_comment_send_rejected(&state.pool, job, error_kind, retry_after_seconds)
                    .await?
            };
            if result == CasResult::Applied {
                Ok(JobOutcome::Failed(error_kind))
            } else {
                Ok(JobOutcome::LeaseLost)
            }
        }
        SendFailure::DeliveryUnknown => {
            let result = mark_post_comment_delivery_unknown(&state.pool, job).await?;
            if result == CasResult::Applied {
                tracing::warn!(job_id = job.id, "post comment delivery outcome is unknown");
                Ok(JobOutcome::Completed)
            } else {
                Ok(JobOutcome::LeaseLost)
            }
        }
    }
}

async fn finalize_completed_post_comment_job(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    state: &AppState,
    job: &PostCommentJob,
    completed: Box<CompletedComment>,
    bot_comment_message_id: i32,
) -> anyhow::Result<JobOutcome> {
    let attempts = serde_json::to_value(&completed.generation.attempts)?;
    let history_used_search_result = completed
        .draft
        .used_search_result_id
        .and_then(|id| completed.search_context.results.get(id.saturating_sub(1)))
        .map(serde_json::to_value)
        .transpose()?;
    let result = finalize_post_comment_sent(
        &state.pool,
        job,
        FinalizePostCommentSent {
            bot_comment_message_id,
            generation: LlmGenerationInsert {
                job_id: job.id,
                provider: &completed.generation.provider,
                model: &completed.generation.model,
                prompt: &completed.prompt_for_log,
                image_used: completed.generation.image_used,
                response: &completed.draft.comment,
                final_html: &completed.final_html,
                attempts: &attempts,
                used_search_result_id: completed.used_search_result_id,
                used_chat_message_ids: &completed.draft.used_chat_message_ids,
            },
            history_used_search_result: history_used_search_result.as_ref(),
            source_channel_id: job.source_channel_id,
            source_message_id: job.source_message_id,
            cleaned_post_text: &job.cleaned_post_text,
            bot_comment: &completed.draft.comment,
        },
    )
    .await?;
    if result == CasResult::LeaseLost {
        return Ok(JobOutcome::LeaseLost);
    }

    if let Some(owner_id) = owner_preview_chat(&state.config) {
        send_owner_preview(
            bot,
            owner_id,
            &completed.final_html,
            MessageId(job.source_message_id),
            &completed.search_context,
            completed.used_search_result_id,
        )
        .await;
    }

    Ok(JobOutcome::Completed)
}

fn ensure_comment_html(final_html: &str, _raw_response: &str) -> anyhow::Result<()> {
    if final_html.trim().is_empty() {
        anyhow::bail!("empty rendered comment from LLM response");
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
    let image_file_id = msg
        .photo()
        .and_then(|photos| photos.iter().max_by_key(|photo| photo.width * photo.height))
        .map(|photo| photo.file.id.0.as_str());
    download_photo_base64(bot, image_file_id, config).await
}

async fn download_photo_base64(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    image_file_id: Option<&str>,
    config: &Config,
) -> anyhow::Result<Option<String>> {
    let Some(image_file_id) = image_file_id else {
        return Ok(None);
    };

    let file = bot.get_file(FileId(image_file_id.to_owned())).await?;
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
