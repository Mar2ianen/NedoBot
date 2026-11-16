use teloxide::prelude::*;

use crate::features::stats::report::{
    send_bottom_messages, send_chat_stats, send_top_messages, send_top_reacted, send_top_word,
    send_user_stats,
};
use crate::features::stats::strings::StatsStrings;
use crate::features::stats::types::StatsPeriod;
use crate::state::AppState;
use crate::telegram::commands::Command;

use super::CommandContext;

pub(super) async fn dispatch(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    command: Command,
) -> ResponseResult<()> {
    let raw_args = match &command {
        Command::Status(args)
        | Command::TopWord(args)
        | Command::UserStats(args)
        | Command::UserStatus(args) => super::super::raw_message_args(msg).unwrap_or(args),
        Command::StatsDay(args)
        | Command::StatsWeek(args)
        | Command::StatsMonth(args)
        | Command::TopMsg(args)
        | Command::TopReact(args)
        | Command::BottomMsg(args) => args,
        _ => unreachable!("only stats commands are routed here"),
    }
    .to_owned();
    let raw_args = raw_args.as_str();
    let context = CommandContext::new(bot, msg, state, raw_args);
    let pool = &context.state.pool;
    let strings = StatsStrings::for_locale(&context.state.config.stats_locale);

    match command {
        Command::StatsDay(args) => {
            send_chat_stats(
                context.bot,
                context.scope.output_chat,
                pool,
                context.scope.stats_chat_id,
                &context.state.render_time,
                StatsPeriod::Day,
                super::super::render_from_message_or_args(context.message, &args),
                &strings,
                context.audience.0,
            )
            .await?;
        }
        Command::StatsWeek(args) => {
            send_chat_stats(
                context.bot,
                context.scope.output_chat,
                pool,
                context.scope.stats_chat_id,
                &context.state.render_time,
                StatsPeriod::Week,
                super::super::render_from_message_or_args(context.message, &args),
                &strings,
                context.audience.0,
            )
            .await?;
        }
        Command::StatsMonth(args) => {
            send_chat_stats(
                context.bot,
                context.scope.output_chat,
                pool,
                context.scope.stats_chat_id,
                &context.state.render_time,
                StatsPeriod::Month,
                super::super::render_from_message_or_args(context.message, &args),
                &strings,
                context.audience.0,
            )
            .await?;
        }
        Command::Status(args) => {
            let period =
                super::super::status_period_from_args(raw_args).unwrap_or(StatsPeriod::Day);
            send_chat_stats(
                context.bot,
                context.scope.output_chat,
                pool,
                context.scope.stats_chat_id,
                &state.render_time,
                period,
                super::super::render_from_message_or_args(context.message, &args),
                &strings,
                context.audience.0,
            )
            .await?;
        }
        Command::TopMsg(args) => {
            send_top_messages(
                context.bot,
                context.scope.output_chat,
                pool,
                context.scope.stats_chat_id,
                super::super::render_from_message_or_args(msg, &args),
                &strings,
                context.audience.0,
            )
            .await?;
        }
        Command::TopWord(_args) => {
            let Some(parsed) = super::super::parse_top_word_args(raw_args) else {
                super::super::send_command_reply_html_for_args(
                    context.bot,
                    context.message,
                    context.state,
                    raw_args,
                    "Использование: /topword <слово> [-r|-p] [-e]. Ищет отдельные вхождения без учета регистра.",
                )
                .await?;
                return Ok(());
            };
            send_top_word(
                context.bot,
                context.scope.output_chat,
                pool,
                context.scope.stats_chat_id,
                &parsed.word,
                parsed.render,
                &strings,
                context.audience.0,
            )
            .await?;
        }
        Command::TopReact(args) => {
            send_top_reacted(
                context.bot,
                context.scope.output_chat,
                pool,
                context.scope.stats_chat_id,
                super::super::render_from_message_or_args(context.message, &args),
                &strings,
                context.audience.0,
            )
            .await?;
        }
        Command::BottomMsg(args) => {
            send_bottom_messages(
                context.bot,
                context.scope.output_chat,
                pool,
                context.scope.stats_chat_id,
                super::super::render_from_message_or_args(context.message, &args),
                &strings,
                context.audience.0,
            )
            .await?;
        }
        Command::UserStats(_target) | Command::UserStatus(_target) => {
            let args = super::super::parse_user_stats_args(raw_args);
            let fallback_user_id = context.actor.reply_user_id.or(context.actor.sender_user_id);
            send_user_stats(
                context.bot,
                context.scope.output_chat,
                pool,
                &context.state.config,
                context.scope.stats_chat_id,
                args.target.as_deref(),
                fallback_user_id,
                args.render,
                &strings,
                context.audience.0,
            )
            .await?;
        }
        _ => unreachable!("only stats commands are routed here"),
    }
    Ok(())
}
