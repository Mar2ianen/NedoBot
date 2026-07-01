use teloxide::{
    dispatching::UpdateFilterExt,
    prelude::*,
    types::{
        ChatId, ChatMemberKind, ChatMemberUpdated, MessageReactionCountUpdated,
        MessageReactionUpdated, ParseMode,
    },
};

mod config;
mod db;
mod features;
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
use features::first_comment::pipeline::maybe_comment_post;
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

    let bot = Bot::from_env().parse_mode(ParseMode::Html);
    let pool = build_pool().await?;
    migrate(&pool).await?;
    let config = Config::from_env();
    if let Err(err) = refresh_known_member_snapshots(&bot, &pool, &config).await {
        tracing::warn!(%err, "failed to refresh member snapshots");
    }
    if let Err(err) = warn_if_reaction_updates_unavailable(&bot, &config).await {
        tracing::warn!(%err, "failed to check reaction update availability");
    }
    let state = AppState::new(pool, config);

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
        .branch(Update::filter_chat_member().endpoint(handle_chat_member));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

async fn handle_message(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    msg: Message,
    state: AppState,
) -> ResponseResult<()> {
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
