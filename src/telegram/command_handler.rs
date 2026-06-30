use teloxide::{prelude::*, utils::command::BotCommands};

use crate::features::first_comment::clean::{clean_post_for_llm, should_generate_comment};
use crate::features::memory::report::send_memory_notes;
use crate::features::stats::types::StatsPeriod;
use crate::state::AppState;
use crate::telegram::commands::Command;
use crate::telegram::custom_emoji::send_custom_emoji_ids;
use crate::telegram::render::send_html;
use crate::{build_comment_html, save_telegram_message, send_chat_stats, send_user_stats};

pub async fn handle_command(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    msg: Message,
    cmd: Command,
    state: AppState,
) -> ResponseResult<()> {
    let pool = &state.pool;
    let config = &state.config;

    if let Err(err) = save_telegram_message(pool, &msg).await {
        tracing::error!(%err, "failed to save command message");
    }

    match cmd {
        Command::Help => {
            bot.send_message(msg.chat.id, Command::descriptions().to_string())
                .await?;
        }
        Command::Ping => {
            bot.send_message(msg.chat.id, "pong").await?;
        }
        Command::Db => {
            let row: (i64,) = sqlx::query_as("select 1")
                .fetch_one(pool)
                .await
                .map_err(|err| {
                    tracing::error!(%err, "database check failed");
                    teloxide::RequestError::Io(std::io::Error::other("database check failed"))
                })?;

            bot.send_message(msg.chat.id, format!("db ok: {}", row.0))
                .await?;
        }
        Command::EmojiIds => {
            send_custom_emoji_ids(&bot, &msg).await?;
        }
        Command::FormatTest(post_text) => {
            if !should_generate_comment(&post_text, config) {
                bot.send_message(
                    msg.chat.id,
                    "Пропускаю: в посте нет сигнатуры обычного поста, похоже на рекламу или служебный пост.",
                )
                .await?;
                return Ok(());
            }

            let clean_post = clean_post_for_llm(&post_text, config);
            let text = build_comment_html(&clean_post, config);
            send_html(&bot, msg.chat.id, text).await?;
        }
        Command::Memory => {
            send_memory_notes(&bot, msg.chat.id, pool).await?;
        }
        Command::StatsDay => {
            send_chat_stats(&bot, msg.chat.id, pool, config, StatsPeriod::Day).await?;
        }
        Command::StatsWeek => {
            send_chat_stats(&bot, msg.chat.id, pool, config, StatsPeriod::Week).await?;
        }
        Command::StatsMonth => {
            send_chat_stats(&bot, msg.chat.id, pool, config, StatsPeriod::Month).await?;
        }
        Command::UserStats(target) => {
            send_user_stats(&bot, msg.chat.id, pool, config, &target).await?;
        }
    }

    Ok(())
}
