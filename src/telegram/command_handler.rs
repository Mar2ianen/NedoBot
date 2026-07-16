use teloxide::{prelude::*, utils::command::BotCommands};

use crate::db::telegram::save_telegram_message;
use crate::features::ask::agent;
use crate::features::ask::notes::{add_chat_note, add_user_note};
use crate::features::ask::rich_markdown;
use crate::features::first_comment::clean::{clean_post_for_llm, should_generate_comment};
use crate::features::first_comment::render::build_comment_html;
use crate::features::memory::report::send_memory_notes;
use crate::features::stats::report::{
    send_chat_stats, send_top_messages, send_top_reacted, send_user_stats,
};
use crate::features::stats::types::{StatsPeriod, StatsRender};
use crate::state::AppState;
use crate::telegram::commands::Command;
use crate::telegram::custom_emoji::send_custom_emoji_ids;
use crate::telegram::render::{escape_html, send_html, send_rich_markdown_reply};

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
            send_html(
                &bot,
                msg.chat.id,
                escape_html(&Command::descriptions().to_string()),
            )
            .await?;
        }
        Command::Ping => {
            bot.send_message(msg.chat.id, "pong").await?;
        }
        Command::Db => {
            let row: (i64,) = sqlx::query_as("select 1::bigint")
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
        Command::Ask(question) => {
            handle_ask_command(&bot, &msg, &state, &question).await?;
        }
        Command::ChatNote(note) => {
            handle_note_command(&bot, &msg, &state, &note, None).await?;
        }
        Command::UserNote(note) => {
            handle_note_command(&bot, &msg, &state, &note, reply_user_id(&msg)).await?;
        }
        Command::StatsDay(args) => {
            let render = render_from_message_or_args(&msg, &args);
            send_chat_stats(&bot, msg.chat.id, pool, config, StatsPeriod::Day, render).await?;
        }
        Command::StatsWeek(args) => {
            let render = render_from_message_or_args(&msg, &args);
            send_chat_stats(&bot, msg.chat.id, pool, config, StatsPeriod::Week, render).await?;
        }
        Command::StatsMonth(args) => {
            let render = render_from_message_or_args(&msg, &args);
            send_chat_stats(&bot, msg.chat.id, pool, config, StatsPeriod::Month, render).await?;
        }
        Command::Status(args) => {
            let raw_args = raw_message_args(&msg).unwrap_or(args.as_str());
            let render = render_from_message_or_args(&msg, &args);
            let period = status_period_from_args(raw_args).unwrap_or(StatsPeriod::Day);
            send_chat_stats(&bot, msg.chat.id, pool, config, period, render).await?;
        }
        Command::TopMsg(args) => {
            send_top_messages(
                &bot,
                msg.chat.id,
                pool,
                config,
                render_from_message_or_args(&msg, &args),
            )
            .await?;
        }
        Command::TopReact(args) => {
            send_top_reacted(
                &bot,
                msg.chat.id,
                pool,
                config,
                render_from_message_or_args(&msg, &args),
            )
            .await?;
        }
        Command::UserStats(target) | Command::UserStatus(target) => {
            let raw_args = raw_message_args(&msg).unwrap_or(target.as_str());
            let render = render_from_message_or_args(&msg, &target);
            let target = strip_render_flag(raw_args);
            let target = target.trim();
            let explicit_target = (!target.is_empty()).then_some(target);
            let fallback_user_id = reply_user_id(&msg).or_else(|| sender_user_id(&msg));

            send_user_stats(
                &bot,
                msg.chat.id,
                pool,
                config,
                explicit_target,
                fallback_user_id,
                render,
            )
            .await?;
        }
    }

    Ok(())
}

async fn handle_note_command(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    note: &str,
    target_user_id: Option<i64>,
) -> ResponseResult<()> {
    let Some(author) = msg.from.as_ref() else {
        return Ok(());
    };
    if msg.chat.id.0 != state.config.discussion_chat_id {
        return Ok(());
    }
    let allowed = state.config.owner_telegram_id == Some(author.id.0 as i64)
        || (state.config.ask_allow_chat_admins
            && bot
                .get_chat_member(msg.chat.id, author.id)
                .await
                .map(|member| member.kind.is_privileged())
                .unwrap_or(false));
    if !allowed {
        return Ok(());
    }
    let result = match target_user_id {
        Some(target_user_id) => {
            add_user_note(
                &state.pool,
                msg.chat.id.0,
                target_user_id,
                author.id.0 as i64,
                note,
            )
            .await
        }
        None => add_chat_note(&state.pool, msg.chat.id.0, author.id.0 as i64, note).await,
    };
    match result {
        Ok(()) => send_html(bot, msg.chat.id, "Заметка сохранена.")
            .await
            .map(|_| ()),
        Err(_) => send_html(
            bot,
            msg.chat.id,
            "Не удалось сохранить заметку: проверь текст и reply для /user_note.",
        )
        .await
        .map(|_| ()),
    }
}

async fn handle_ask_command(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    question: &str,
) -> ResponseResult<()> {
    let config = &state.config;
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };
    if !config.ask_enabled || msg.chat.id.0 != config.discussion_chat_id {
        return Ok(());
    }
    let is_owner = config.owner_telegram_id == Some(user.id.0 as i64);
    let is_admin = config.ask_allow_chat_admins
        && bot
            .get_chat_member(msg.chat.id, user.id)
            .await
            .map(|member| member.kind.is_privileged())
            .unwrap_or(false);
    if !is_owner && !is_admin {
        send_html(
            bot,
            msg.chat.id,
            "Команда /ask пока доступна владельцу и администраторам.",
        )
        .await?;
        return Ok(());
    }
    if question.trim().is_empty() {
        send_html(bot, msg.chat.id, "Напиши вопрос: /ask <вопрос>.").await?;
        return Ok(());
    }

    let permit =
        state.ask_slots.clone().try_acquire_owned().map_err(|_| {
            teloxide::RequestError::Io(std::io::Error::other("ask assistant is busy"))
        })?;
    let reply_context = msg
        .reply_to_message()
        .and_then(|reply| reply.text().or_else(|| reply.caption()));
    let answer = agent::answer(config, question, reply_context)
        .await
        .map_err(|err| {
            tracing::warn!(%err, "ask assistant failed");
            teloxide::RequestError::Io(std::io::Error::other("ask assistant failed"))
        });
    drop(permit);
    match answer {
        Ok(answer) => {
            let markdown = rich_markdown::validate(&answer).map_err(|err| {
                tracing::warn!(%err, "ask assistant returned unsafe markdown");
                teloxide::RequestError::Io(std::io::Error::other("ask markdown is invalid"))
            })?;
            if send_rich_markdown_reply(msg.chat.id, msg.id, markdown)
                .await
                .is_ok()
            {
                Ok(())
            } else {
                send_html(bot, msg.chat.id, escape_html(&answer))
                    .await
                    .map(|_| ())
            }
        }
        Err(_) => send_html(
            bot,
            msg.chat.id,
            "Не смог подготовить ответ. Попробуй ещё раз чуть позже.",
        )
        .await
        .map(|_| ()),
    }
}

pub async fn handle_reply_user_stats_command(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    msg: Message,
    state: AppState,
) -> ResponseResult<bool> {
    if !is_bare_userstats_command(&msg) {
        return Ok(false);
    }

    let pool = &state.pool;
    let config = &state.config;

    if let Err(err) = save_telegram_message(pool, &msg).await {
        tracing::error!(%err, "failed to save command message");
    }

    let render = msg
        .text()
        .or_else(|| msg.caption())
        .map(render_from_args)
        .unwrap_or(StatsRender::Rich);

    send_user_stats(
        &bot,
        msg.chat.id,
        pool,
        config,
        None,
        reply_user_id(&msg).or_else(|| sender_user_id(&msg)),
        render,
    )
    .await?;

    Ok(true)
}

fn reply_user_id(msg: &Message) -> Option<i64> {
    msg.reply_to_message()
        .and_then(|reply| reply.from.as_ref())
        .map(|user| user.id.0 as i64)
}

fn sender_user_id(msg: &Message) -> Option<i64> {
    msg.from.as_ref().map(|user| user.id.0 as i64)
}

fn is_bare_userstats_command(msg: &Message) -> bool {
    let Some(text) = msg.text().or_else(|| msg.caption()) else {
        return false;
    };

    let mut parts = text.split_whitespace();
    let command = parts.next().unwrap_or_default();
    if parts.next().is_some() {
        return false;
    }

    matches!(command, "/userstats" | "/userstatus")
        || command
            .strip_prefix("/userstats@")
            .or_else(|| command.strip_prefix("/userstatus@"))
            .is_some_and(|bot_name| !bot_name.is_empty())
}

fn render_from_message_or_args(msg: &Message, args: &str) -> StatsRender {
    raw_message_args(msg)
        .filter(|raw_args| has_render_flag(raw_args))
        .map(render_from_args)
        .unwrap_or_else(|| render_from_args(args))
}

fn raw_message_args(msg: &Message) -> Option<&str> {
    msg.text()
        .or_else(|| msg.caption())
        .and_then(raw_command_args)
}

fn render_from_args(args: &str) -> StatsRender {
    if args.split_whitespace().any(is_plain_render_flag) {
        StatsRender::Html
    } else if args.split_whitespace().any(is_rich_render_flag) {
        StatsRender::Rich
    } else {
        StatsRender::Rich
    }
}

fn has_render_flag(args: &str) -> bool {
    args.split_whitespace()
        .any(|part| is_rich_render_flag(part) || is_plain_render_flag(part))
}

fn is_rich_render_flag(part: &str) -> bool {
    matches!(part, "-r" | "--rich")
}

fn is_plain_render_flag(part: &str) -> bool {
    matches!(part, "-p" | "--plain" | "--poor")
}

fn raw_command_args(text: &str) -> Option<&str> {
    let mut parts = text.trim().splitn(2, char::is_whitespace);
    parts.next()?;
    Some(parts.next().unwrap_or_default().trim())
}

fn strip_render_flag(args: &str) -> String {
    args.split_whitespace()
        .filter(|part| !is_rich_render_flag(part) && !is_plain_render_flag(part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn status_period_from_args(args: &str) -> Option<StatsPeriod> {
    strip_render_flag(args)
        .split_whitespace()
        .next()
        .and_then(|period| match period.to_lowercase().as_str() {
            "day" | "daily" | "день" | "дня" => Some(StatsPeriod::Day),
            "week" | "weekly" | "неделя" | "неделю" => Some(StatsPeriod::Week),
            "month" | "monthly" | "месяц" => Some(StatsPeriod::Month),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rich_and_forced_plain_flags() {
        assert!(matches!(render_from_args("-r"), StatsRender::Rich));
        assert!(matches!(render_from_args("week --rich"), StatsRender::Rich));
        assert!(matches!(render_from_args("week"), StatsRender::Rich));
        assert!(matches!(render_from_args("week -p"), StatsRender::Html));
        assert!(matches!(
            render_from_args("--rich --poor"),
            StatsRender::Html
        ));
    }

    #[test]
    fn reads_raw_command_args_from_full_message_text() {
        assert_eq!(raw_command_args("/stats_day -r"), Some("-r"));
        assert_eq!(
            raw_command_args("/userstats 445144708 -r"),
            Some("445144708 -r")
        );
        assert_eq!(raw_command_args("/topmsg"), Some(""));
    }

    #[test]
    fn strips_render_flags_from_user_target() {
        assert_eq!(strip_render_flag("@Chechulinm -r"), "@Chechulinm");
        assert_eq!(strip_render_flag("-r 445144708"), "445144708");
        assert_eq!(strip_render_flag("445144708 --poor"), "445144708");
        assert_eq!(strip_render_flag("--plain @Chechulinm"), "@Chechulinm");
    }

    #[test]
    fn parses_status_period() {
        assert!(matches!(
            status_period_from_args("week -r"),
            Some(StatsPeriod::Week)
        ));
        assert!(matches!(
            status_period_from_args("месяц"),
            Some(StatsPeriod::Month)
        ));
    }
}
