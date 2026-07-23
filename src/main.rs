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
    mark_user_profile_refresh_error, refresh_known_member_snapshots, save_chat_member_event,
    save_edited_telegram_message, save_message_reaction, save_message_reaction_count,
    user_profile_needs_refresh,
};
use db::{build_pool, migrate};
use features::avatar_analysis::service::{
    enqueue_current_avatar_analysis, process_next_avatar_analysis_job,
};
use features::first_comment::pipeline::maybe_comment_post;
use features::first_message_spam::{
    enqueue_first_message_spam_analysis, process_next_first_message_spam_analysis_job,
};
use features::memory::service::process_next_history_entry;
use features::new_user_analysis::analyze_new_user_profile;
use features::spam_review::{apply_callback, create_high_risk_review, parse_callback, send_review};
use features::user_profiles::service::refresh_profile;
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

    let config = Config::from_env();
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
    spawn_avatar_analysis_worker(bot.inner().clone(), state.clone());
    spawn_first_message_spam_analysis_worker(bot.inner().clone(), state.clone());
    spawn_post_history_worker(state.clone());

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
        .dependencies(dptree::deps![state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

fn spawn_post_history_worker(state: AppState) {
    if !state.config.rag_enabled {
        return;
    }
    tokio::spawn(async move {
        loop {
            match process_next_history_entry(&state.pool, &state.config).await {
                Ok(true) => continue,
                Ok(false) => tokio::time::sleep(std::time::Duration::from_secs(5)).await,
                Err(err) => {
                    tracing::warn!(%err, "post history worker failed");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    });
}

async fn handle_message(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    msg: Message,
    state: AppState,
) -> ResponseResult<()> {
    spawn_message_author_profile_refresh(&bot, &msg, &state);

    if handle_reply_user_stats_command(bot.clone(), msg.clone(), state.clone()).await? {
        return Ok(());
    }

    match maybe_transcribe_voice(&bot, &msg, &state).await {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(err) => tracing::error!(%err, "failed to process voice transcription"),
    }

    if let Err(err) = maybe_comment_post(&bot, &msg, &state).await {
        tracing::error!(%err, "failed to process message");
    }

    Ok(())
}

fn spawn_message_author_profile_refresh(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
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
    let chat_id = msg.chat.id.0;
    let bot = bot.inner().clone();
    let pool = state.pool.clone();
    let profile_refresh_slots = state.profile_refresh_slots.clone();
    let avatar_classifier_enabled = state.config.avatar_classifier_enabled;
    tokio::spawn(async move {
        match user_profile_needs_refresh(&pool, user_id).await {
            Ok(true) => {}
            Ok(false) => return,
            Err(err) => {
                tracing::warn!(%err, user_id, "failed to check user profile refresh state");
                return;
            }
        }

        let _permit = match profile_refresh_slots.acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                tracing::warn!(user_id, "profile refresh limiter is closed");
                return;
            }
        };

        match refresh_profile(&bot, &pool, user_id).await {
            Ok(()) => {
                if let Err(err) = analyze_new_user_profile(&pool, chat_id, user_id).await {
                    tracing::warn!(%err, user_id, "failed to analyze new user profile");
                } else {
                    if let Err(err) =
                        enqueue_first_message_spam_analysis(&pool, chat_id, user_id).await
                    {
                        tracing::warn!(%err, user_id, "failed to enqueue first-message spam analysis");
                    }
                    match create_high_risk_review(&pool, chat_id, user_id).await {
                        Ok(Some(review)) => {
                            if let Err(err) = send_review(&bot, &review).await {
                                tracing::warn!(%err, user_id, "failed to send spam review");
                            }
                        }
                        Ok(None) => {}
                        Err(err) => tracing::warn!(%err, user_id, "failed to create spam review"),
                    }
                }
                if avatar_classifier_enabled
                    && let Err(err) = enqueue_current_avatar_analysis(&pool, user_id).await
                {
                    tracing::warn!(%err, user_id, "failed to enqueue avatar analysis");
                }
            }
            Err(err) => {
                let message = err.to_string();
                if let Err(save_err) =
                    mark_user_profile_refresh_error(&pool, user_id, &message).await
                {
                    tracing::warn!(%save_err, user_id, "failed to save profile refresh error");
                }
                tracing::warn!(%err, user_id, "failed to refresh message author profile");
            }
        }
    });
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
                Ok(false) => tokio::time::sleep(std::time::Duration::from_secs(5)).await,
                Err(err) => {
                    tracing::warn!(%err, "avatar analysis worker failed to claim a job");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    });
}

fn spawn_first_message_spam_analysis_worker(bot: Bot, state: AppState) {
    tokio::spawn(async move {
        loop {
            match process_next_first_message_spam_analysis_job(&bot, &state.pool, &state.config)
                .await
            {
                Ok(true) => continue,
                Ok(false) => tokio::time::sleep(std::time::Duration::from_secs(5)).await,
                Err(err) => {
                    tracing::warn!(%err, "first-message spam worker failed to claim a job");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
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
    if let Err(err) = save_edited_telegram_message(&state.pool, &msg).await {
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
