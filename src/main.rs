use teloxide::{
    dispatching::UpdateFilterExt,
    prelude::*,
    types::{
        ChatId, ChatMemberKind, ChatMemberUpdated, MessageReactionCountUpdated,
        MessageReactionUpdated, ParseMode,
    },
};

#[cfg(feature = "moderation")]
use teloxide::types::CallbackQuery;

mod community;
mod config;
mod config_file;
mod db;
mod features;
mod http;
mod llm;
mod state;
mod telegram;
mod text;

use config::Config;
use db::telegram::{
    refresh_known_member_snapshots, save_chat_member_event, save_edited_telegram_message,
    save_message_reaction, save_message_reaction_count,
};
use db::{build_pool, migrate};
use features::chat_retrieval::process_next_embedding_batch;
#[cfg(feature = "auto-comment")]
use features::first_comment::pipeline::{maybe_comment_post, process_next_post_comment_job};
use features::ingest::{ingest_message, is_managed_chat, managed_chat_allows};
#[cfg(feature = "moderation")]
use features::jobs::policy::EXTERNAL_ANALYSIS_POLL;
use features::jobs::policy::POST_HISTORY_POLL;
#[cfg(feature = "voice")]
use features::jobs::policy::VOICE_TRANSCRIPTION_POLL;
use features::memory::service::process_next_history_entry;
#[cfg(feature = "moderation")]
use features::new_user_audit::service::process_next_new_user_audit_job;
#[cfg(feature = "moderation")]
use features::spam_review::{apply_callback, parse_callback, process_next_review_delivery};
use features::user_profiles::enrichment::{
    ProfileRefreshEnqueueResult, ProfileRefreshQueue, spawn_profile_refresh_workers,
};
#[cfg(feature = "voice")]
use features::voice::pipeline::{maybe_transcribe_voice, process_next_voice_job};
use llm::genai_transport::GenAiTransport;
use state::AppState;
use telegram::command_handler::{handle_command, handle_reply_user_stats_command};
use telegram::commands::Command;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let explicit_env_file = std::env::var("ENV_FILE").ok();
    let env_file = explicit_env_file.as_deref().unwrap_or(".env");
    if explicit_env_file.is_some() {
        dotenvy::from_filename(env_file)
            .map_err(|error| anyhow::anyhow!("failed to load {env_file}: {error}"))?;
    } else {
        dotenvy::dotenv().ok();
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,teloxide=info".into()),
        )
        .init();

    let config = Config::from_env()?;
    config.validate_runtime_secrets()?;
    tracing::info!(
        instance_id = %config.community.instance.id,
        instance_name = %config.community.instance.display_name,
        risk_profile = ?config.community.moderation.risk_profile,
        "starting configured community instance"
    );
    for chat_id in config.managed_chat_ids() {
        if let Some(chat) = config.chat_by_id(chat_id) {
            tracing::info!(chat_key = %chat.key, chat_id, "managed Telegram chat");
        }
    }
    GenAiTransport::cached(config.llm_proxy_url.as_deref())?;
    let bot = Bot::from_env().parse_mode(ParseMode::Html);
    preflight_managed_chats(&bot, &config).await?;
    let pool = build_pool().await?;
    migrate(&pool).await?;
    if let Err(err) = refresh_known_member_snapshots(&bot, &pool, &config).await {
        tracing::warn!(%err, "failed to refresh member snapshots");
    }
    if let Err(err) = warn_if_reaction_updates_unavailable(&bot, &config).await {
        tracing::warn!(%err, "failed to check reaction update availability");
    }
    let state = AppState::new(pool, config);
    let profile_refresh_queue = spawn_profile_refresh_workers(
        bot.inner().clone(),
        state.pool.clone(),
        state.config.clone(),
    );
    #[cfg(feature = "moderation")]
    if state.config.new_user_audit_enabled {
        spawn_new_user_audit_worker(bot.inner().clone(), state.clone());
    }
    #[cfg(feature = "moderation")]
    if state.config.community.moderation.enabled {
        spawn_spam_review_delivery_worker(bot.inner().clone(), state.clone());
    }
    #[cfg(feature = "auto-comment")]
    if state.config.community.first_comment.enabled {
        spawn_post_comment_worker(bot.clone(), state.clone());
    }
    spawn_post_history_worker(state.clone());
    spawn_chat_retrieval_embedding_worker(state.clone());
    #[cfg(feature = "voice")]
    spawn_voice_transcription_worker(bot.clone(), state.clone());

    let mut handler = dptree::entry()
        .branch(
            Update::filter_message()
                .branch(
                    dptree::entry()
                        .filter_command::<Command>()
                        .endpoint(handle_command),
                )
                .branch(dptree::endpoint(handle_message)),
        )
        .branch(Update::filter_message_reaction_updated().endpoint(handle_message_reaction))
        .branch(
            Update::filter_message_reaction_count_updated().endpoint(handle_message_reaction_count),
        )
        .branch(Update::filter_edited_message().endpoint(handle_edited_message));
    #[cfg(feature = "moderation")]
    {
        handler = handler.branch(Update::filter_callback_query().endpoint(handle_callback_query));
    }
    let handler = handler.branch(Update::filter_chat_member().endpoint(handle_chat_member));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state, profile_refresh_queue])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

#[cfg(feature = "auto-comment")]
fn spawn_post_comment_worker(bot: teloxide::adaptors::DefaultParseMode<Bot>, state: AppState) {
    tokio::spawn(async move {
        loop {
            match process_next_post_comment_job(&bot, &state).await {
                Ok(true) => continue,
                Ok(false) => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
                Err(err) => {
                    tracing::warn!(%err, "post comment worker failed to claim a job");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
        }
    });
}

fn spawn_post_history_worker(state: AppState) {
    if !state.config.rag_enabled {
        return;
    }
    tokio::spawn(async move {
        loop {
            match process_next_history_entry(&state.pool, &state.config).await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        POST_HISTORY_POLL.idle_seconds(),
                    ))
                    .await
                }
                Err(err) => {
                    tracing::warn!(%err, "post history worker failed");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        POST_HISTORY_POLL.error_seconds(),
                    ))
                    .await;
                }
            }
        }
    });
}

fn spawn_chat_retrieval_embedding_worker(state: AppState) {
    if !state.config.chat_retrieval_embeddings_enabled {
        return;
    }
    tokio::spawn(async move {
        loop {
            match process_next_embedding_batch(&state.pool, &state.config).await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        state.config.chat_retrieval_embedding_poll_sec,
                    ))
                    .await;
                }
                Err(err) => {
                    tracing::warn!(%err, "chat retrieval embedding worker failed");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        state.config.chat_retrieval_embedding_poll_sec,
                    ))
                    .await;
                }
            }
        }
    });
}

#[cfg(feature = "voice")]
fn spawn_voice_transcription_worker(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    state: AppState,
) {
    if !state.config.voice_transcription_enabled {
        return;
    }
    tokio::spawn(async move {
        loop {
            match process_next_voice_job(&bot, &state).await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        VOICE_TRANSCRIPTION_POLL.idle_seconds(),
                    ))
                    .await;
                }
                Err(err) => {
                    tracing::warn!(%err, "voice transcription worker failed");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        VOICE_TRANSCRIPTION_POLL.error_seconds(),
                    ))
                    .await;
                }
            }
        }
    });
}

async fn handle_message(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    msg: Message,
    state: AppState,
    profile_refresh_queue: ProfileRefreshQueue,
) -> ResponseResult<()> {
    if !is_managed_chat(&state.config, msg.chat.id.0) {
        tracing::debug!(chat_id = msg.chat.id.0, "ignored message from unknown chat");
        return Ok(());
    }
    match ingest_message(&state.pool, &msg, &state.config).await {
        Ok(true) => {}
        Ok(false) => return Ok(()),
        Err(err) => {
            tracing::error!(%err, chat_id = msg.chat.id.0, "failed to ingest message");
            return Ok(());
        }
    }
    enqueue_message_author_profile_refresh(&msg, &state, &profile_refresh_queue);

    if handle_reply_user_stats_command(bot.clone(), msg.clone(), state.clone()).await? {
        return Ok(());
    }

    #[cfg(feature = "voice")]
    match maybe_transcribe_voice(&bot, &msg, &state).await {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(err) => tracing::error!(%err, "failed to process voice transcription"),
    }

    #[cfg(feature = "auto-comment")]
    if let Err(err) = maybe_comment_post(&msg, &state).await {
        tracing::error!(%err, "failed to process message");
    }

    Ok(())
}

fn enqueue_message_author_profile_refresh(
    msg: &Message,
    state: &AppState,
    profile_refresh_queue: &ProfileRefreshQueue,
) {
    if !managed_chat_allows(&state.config, msg.chat.id.0, |chat| chat.ingest)
        || msg.is_automatic_forward()
    {
        return;
    }

    let Some(user) = msg.from.as_ref() else {
        return;
    };
    if user.is_bot {
        return;
    }

    let user_id = user.id.0 as i64;
    match profile_refresh_queue.try_enqueue(msg.chat.id.0, user_id) {
        ProfileRefreshEnqueueResult::Queued => {}
        ProfileRefreshEnqueueResult::Coalesced => {
            tracing::debug!(user_id, "coalesced duplicate profile refresh event");
        }
        ProfileRefreshEnqueueResult::Full => {
            tracing::warn!(
                user_id,
                "skipped profile refresh because bounded queue is full"
            );
        }
        ProfileRefreshEnqueueResult::Closed => {
            tracing::warn!(user_id, "skipped profile refresh because queue is closed");
        }
    }
}

#[cfg(feature = "moderation")]
async fn handle_callback_query(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    query: CallbackQuery,
    state: AppState,
) -> ResponseResult<()> {
    let reviewer_id = query.from.id.0 as i64;
    if !state.config.reviewer_user_ids().contains(&reviewer_id) {
        bot.answer_callback_query(query.id)
            .text("Недостаточно прав.")
            .await?;
        return Ok(());
    }
    let Some((request_id, decision)) = query.data.as_deref().and_then(parse_callback) else {
        return Ok(());
    };
    match apply_callback(&state.pool, request_id, decision, reviewer_id).await {
        Ok(Some(text)) => {
            bot.answer_callback_query(query.id.clone())
                .text(text)
                .await?;
            if let Some(message) = query.regular_message() {
                bot.delete_message(message.chat.id, message.id).await?;
            }
        }
        Ok(None) => {
            bot.answer_callback_query(query.id)
                .text("Решение уже принято или кнопка устарела.")
                .await?;
        }
        Err(err) => {
            tracing::error!(%err, request_id, "failed to apply spam review callback");
            bot.answer_callback_query(query.id)
                .text("Не удалось сохранить решение.")
                .await?;
        }
    }
    Ok(())
}

#[cfg(feature = "moderation")]
fn spawn_new_user_audit_worker(bot: Bot, state: AppState) {
    tokio::spawn(async move {
        loop {
            match process_next_new_user_audit_job(&bot, &state.pool, &state.config).await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.idle_seconds(),
                    ))
                    .await
                }
                Err(err) => {
                    tracing::warn!(%err, "unified new user audit worker failed to claim a job");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.error_seconds(),
                    ))
                    .await;
                }
            }
        }
    });
}

#[cfg(feature = "moderation")]
fn spawn_spam_review_delivery_worker(bot: Bot, state: AppState) {
    tokio::spawn(async move {
        loop {
            match process_next_review_delivery(&bot, &state.pool, &state.config).await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.idle_seconds(),
                    ))
                    .await
                }
                Err(err) => {
                    tracing::warn!(%err, "spam review delivery worker failed");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.error_seconds(),
                    ))
                    .await;
                }
            }
        }
    });
}

async fn handle_message_reaction(
    reaction: MessageReactionUpdated,
    state: AppState,
) -> ResponseResult<()> {
    if !managed_chat_allows(&state.config, reaction.chat.id.0, |chat| chat.ingest) {
        return Ok(());
    }
    if let Err(err) = save_message_reaction(&state.pool, &reaction).await {
        tracing::error!(%err, "failed to save message reaction");
    }

    Ok(())
}

async fn handle_message_reaction_count(
    reaction_count: MessageReactionCountUpdated,
    state: AppState,
) -> ResponseResult<()> {
    if !managed_chat_allows(&state.config, reaction_count.chat.id.0, |chat| chat.ingest) {
        return Ok(());
    }
    if let Err(err) = save_message_reaction_count(&state.pool, &reaction_count).await {
        tracing::error!(%err, "failed to save message reaction count");
    }

    Ok(())
}

async fn handle_edited_message(msg: Message, state: AppState) -> ResponseResult<()> {
    if !managed_chat_allows(&state.config, msg.chat.id.0, |chat| chat.ingest) {
        return Ok(());
    }
    if let Err(err) = save_edited_telegram_message(&state.pool, &msg, &state.config).await {
        tracing::error!(%err, "failed to save edited message");
    }

    Ok(())
}

async fn handle_chat_member(member: ChatMemberUpdated, state: AppState) -> ResponseResult<()> {
    if !managed_chat_allows(&state.config, member.chat.id.0, |chat| chat.ingest) {
        return Ok(());
    }
    if let Err(err) = save_chat_member_event(&state.pool, &member).await {
        tracing::error!(%err, "failed to save chat member event");
    }

    Ok(())
}

async fn warn_if_reaction_updates_unavailable(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    config: &Config,
) -> anyhow::Result<()> {
    let me = bot.get_me().await?;
    for chat_id in config.managed_chat_ids() {
        let member = bot.get_chat_member(ChatId(chat_id), me.id).await?;
        if !matches!(
            member.kind,
            ChatMemberKind::Administrator(_) | ChatMemberKind::Owner(_)
        ) {
            tracing::warn!(
                chat_id,
                status = ?member.kind,
                "bot is not chat administrator; Telegram will not send message_reaction updates"
            );
        }
    }

    Ok(())
}

async fn preflight_managed_chats(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    config: &Config,
) -> anyhow::Result<()> {
    for chat_id in config.managed_chat_ids() {
        bot.get_chat(ChatId(chat_id))
            .await
            .map_err(|error| anyhow::anyhow!("managed chat {chat_id} preflight failed: {error}"))?;
    }
    Ok(())
}
