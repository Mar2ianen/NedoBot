use teloxide::{
    dispatching::UpdateFilterExt,
    prelude::*,
    types::{
        CallbackQuery, ChatId, ChatMemberKind, ChatMemberUpdated, MessageReactionCountUpdated,
        MessageReactionUpdated, ParseMode,
    },
};

mod config;
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
use features::avatar_analysis::service::process_next_avatar_analysis_job;
use features::chat_retrieval::process_next_embedding_batch;
use features::first_comment::pipeline::{maybe_comment_post, process_next_post_comment_job};
use features::first_message_spam::process_next_first_message_spam_analysis_job;
use features::jobs::policy::{EXTERNAL_ANALYSIS_POLL, POST_HISTORY_POLL};
use features::memory::service::process_next_history_entry;
use features::new_user_audit::service::process_next_new_user_audit_job;
use features::spam_review::{apply_callback, parse_callback};
use features::user_profiles::enrichment::{
    ProfileRefreshEnqueueResult, ProfileRefreshQueue, spawn_profile_refresh_workers,
};
use features::voice::pipeline::maybe_transcribe_voice;
use state::AppState;
use telegram::command_handler::{handle_command, handle_reply_user_stats_command};
use telegram::commands::Command;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,teloxide=info".into()),
        )
        .init();

    let config = Config::from_env()?;
    config.validate_runtime_secrets()?;
    let bot = Bot::from_env().parse_mode(ParseMode::Html);
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
    if state.config.new_user_audit_enabled {
        // В shadow-режиме worker сохраняет assessment; authoritative режим materialize-ит score/review.
        spawn_new_user_audit_worker(bot.inner().clone(), state.clone());
    }
    // Legacy avatar worker остаётся authoritative в shadow-режиме unified audit.
    spawn_avatar_analysis_worker(bot.inner().clone(), state.clone());
    // Delivery review-карточек не зависит от optional first-message analysis.
    spawn_first_message_spam_analysis_worker(bot.inner().clone(), state.clone());
    spawn_post_comment_worker(bot.clone(), state.clone());
    spawn_post_history_worker(state.clone());
    spawn_chat_retrieval_embedding_worker(state.clone());

    let handler = dptree::entry()
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
        .branch(Update::filter_edited_message().endpoint(handle_edited_message))
        .branch(Update::filter_callback_query().endpoint(handle_callback_query))
        .branch(Update::filter_chat_member().endpoint(handle_chat_member));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state, profile_refresh_queue])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

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

async fn handle_message(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    msg: Message,
    state: AppState,
    profile_refresh_queue: ProfileRefreshQueue,
) -> ResponseResult<()> {
    enqueue_message_author_profile_refresh(&msg, &state, &profile_refresh_queue);

    if handle_reply_user_stats_command(bot.clone(), msg.clone(), state.clone()).await? {
        return Ok(());
    }

    match maybe_transcribe_voice(&bot, &msg, &state).await {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(err) => tracing::error!(%err, "failed to process voice transcription"),
    }

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
    if msg.chat.id.0 != state.config.discussion_chat_id || msg.is_automatic_forward() {
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

async fn handle_callback_query(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    query: CallbackQuery,
    state: AppState,
) -> ResponseResult<()> {
    let Some(owner_id) = state.config.owner_telegram_id else {
        return Ok(());
    };
    if query.from.id.0 as i64 != owner_id {
        bot.answer_callback_query(query.id)
            .text("Недостаточно прав.")
            .await?;
        return Ok(());
    }
    let Some((request_id, decision)) = query.data.as_deref().and_then(parse_callback) else {
        return Ok(());
    };
    match apply_callback(&state.pool, request_id, decision, owner_id).await {
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

fn spawn_avatar_analysis_worker(bot: Bot, state: AppState) {
    if !state.config.avatar_classifier_enabled {
        return;
    }
    tokio::spawn(async move {
        loop {
            let permit = match state.avatar_classifier_slots.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            let processed =
                process_next_avatar_analysis_job(&bot, &state.pool, &state.config).await;
            drop(permit);
            match processed {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.idle_seconds(),
                    ))
                    .await
                }
                Err(err) => {
                    tracing::warn!(%err, "avatar analysis worker failed to claim a job");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.error_seconds(),
                    ))
                    .await;
                }
            }
        }
    });
}

fn spawn_first_message_spam_analysis_worker(bot: Bot, state: AppState) {
    // Review-card delivery is independent from optional LLM first-message analysis.
    // The worker therefore remains active to retry pending Telegram notifications.
    tokio::spawn(async move {
        loop {
            match process_next_first_message_spam_analysis_job(&bot, &state.pool, &state.config)
                .await
            {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.idle_seconds(),
                    ))
                    .await
                }
                Err(err) => {
                    tracing::warn!(%err, "spam review or first-message analysis worker failed");
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
    if let Err(err) = save_message_reaction(&state.pool, &reaction).await {
        tracing::error!(%err, "failed to save message reaction");
    }

    Ok(())
}

async fn handle_message_reaction_count(
    reaction_count: MessageReactionCountUpdated,
    state: AppState,
) -> ResponseResult<()> {
    if let Err(err) = save_message_reaction_count(&state.pool, &reaction_count).await {
        tracing::error!(%err, "failed to save message reaction count");
    }

    Ok(())
}

async fn handle_edited_message(msg: Message, state: AppState) -> ResponseResult<()> {
    if let Err(err) = save_edited_telegram_message(&state.pool, &msg, &state.config).await {
        tracing::error!(%err, "failed to save edited message");
    }

    Ok(())
}

async fn handle_chat_member(member: ChatMemberUpdated, state: AppState) -> ResponseResult<()> {
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
    let member = bot
        .get_chat_member(ChatId(config.discussion_chat_id), me.id)
        .await?;

    if !matches!(
        member.kind,
        ChatMemberKind::Administrator(_) | ChatMemberKind::Owner(_)
    ) {
        tracing::warn!(
            status = ?member.kind,
            "bot is not chat administrator; Telegram will not send message_reaction updates"
        );
    }

    Ok(())
}
