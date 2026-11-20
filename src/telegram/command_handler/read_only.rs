use teloxide::prelude::*;

use crate::features::memory::report::send_memory_notes;
use crate::state::AppState;
use crate::telegram::commands::Command;
use crate::telegram::render::escape_html;
use crate::telegram::service_messages::MessageAudience;

use super::{command_audience, command_descriptions, send_command_html};

mod stats;

/// Dispatch read-only utility and statistics commands. Returning `Some` keeps
/// the existing general command catalog authoritative for every other command.
pub(super) async fn dispatch(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    command: Command,
) -> ResponseResult<Option<Command>> {
    match route_kind(&command) {
        None => Ok(Some(command)),
        Some(ReadOnlyRoute::Utility) => {
            dispatch_utility(bot, msg, state, command).await?;
            Ok(None)
        }
        Some(ReadOnlyRoute::Statistics) => {
            stats::dispatch(bot, msg, state, command).await?;
            Ok(None)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadOnlyRoute {
    Utility,
    Statistics,
}

fn route_kind(command: &Command) -> Option<ReadOnlyRoute> {
    match command {
        Command::Help | Command::Ping | Command::Db | Command::Memory => {
            Some(ReadOnlyRoute::Utility)
        }
        Command::StatsDay(_)
        | Command::StatsWeek(_)
        | Command::StatsMonth(_)
        | Command::Status(_)
        | Command::TopMsg(_)
        | Command::TopWord(_)
        | Command::TopReact(_)
        | Command::BottomMsg(_)
        | Command::UserStats(_)
        | Command::UserStatus(_) => Some(ReadOnlyRoute::Statistics),
        _ => None,
    }
}

async fn dispatch_utility(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    command: Command,
) -> ResponseResult<()> {
    match command {
        Command::Help => {
            let descriptions = command_descriptions(&state.config, msg.chat.id.0);
            send_command_html(bot, msg, state, escape_html(&descriptions)).await?;
        }
        Command::Ping => {
            send_command_html(bot, msg, state, "pong").await?;
        }
        Command::Db => {
            let row: (i64,) = sqlx::query_as("select 1::bigint")
                .fetch_one(&state.pool)
                .await
                .map_err(|err| {
                    tracing::error!(%err, "database check failed");
                    teloxide::RequestError::Io(
                        std::io::Error::other("database check failed").into(),
                    )
                })?;
            send_command_html(bot, msg, state, format!("db ok: {}", row.0)).await?;
        }
        Command::Memory => {
            send_memory_notes(bot, msg.chat.id, &state.pool, command_audience(msg, state)).await?;
        }
        _ => unreachable!("only utility commands are routed here"),
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct ChatScope {
    output_chat: ChatId,
    stats_chat_id: i64,
}

#[derive(Clone, Copy)]
struct Actor {
    reply_user_id: Option<i64>,
    sender_user_id: Option<i64>,
}

#[derive(Clone, Copy)]
struct Audience(Option<MessageAudience>);

struct CommandContext<'a> {
    bot: &'a teloxide::adaptors::DefaultParseMode<Bot>,
    message: &'a Message,
    state: &'a AppState,
    scope: ChatScope,
    actor: Actor,
    audience: Audience,
}

impl<'a> CommandContext<'a> {
    fn new(
        bot: &'a teloxide::adaptors::DefaultParseMode<Bot>,
        message: &'a Message,
        state: &'a AppState,
        args: &str,
    ) -> Self {
        Self {
            bot,
            message,
            state,
            scope: ChatScope {
                output_chat: message.chat.id,
                stats_chat_id: message.chat.id.0,
            },
            actor: Actor {
                reply_user_id: super::reply_user_id(message),
                sender_user_id: super::sender_user_id(message),
            },
            audience: Audience(super::command_audience_for_args(message, state, args)),
        }
    }
}

#[cfg(test)]
mod tests {
    use teloxide::utils::command::BotCommands;

    use super::{ReadOnlyRoute, route_kind};
    use crate::telegram::commands::Command;

    #[test]
    fn read_only_command_dispatch_table_preserves_aliases_and_arg_strings() {
        let cases = [
            ("/help", ReadOnlyRoute::Utility),
            ("/ping", ReadOnlyRoute::Utility),
            ("/db", ReadOnlyRoute::Utility),
            ("/memory", ReadOnlyRoute::Utility),
            ("/stats_day -p -e", ReadOnlyRoute::Statistics),
            ("/status week -r", ReadOnlyRoute::Statistics),
            ("/topword благодарю -p", ReadOnlyRoute::Statistics),
            ("/userstats @vasya -e", ReadOnlyRoute::Statistics),
            ("/userstatus 123 -r", ReadOnlyRoute::Statistics),
        ];

        for (text, expected_route) in cases {
            let command = Command::parse(text, "nedobot")
                .unwrap_or_else(|error| panic!("failed to parse {text}: {error}"));
            assert_eq!(route_kind(&command), Some(expected_route), "{text}");
        }

        let command = Command::parse("/emoji_ids", "nedobot").expect("utility command parses");
        assert_eq!(route_kind(&command), None);
    }
}
