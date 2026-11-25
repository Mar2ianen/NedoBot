use teloxide::prelude::*;

use crate::state::AppState;
use crate::telegram::commands::Command;

#[cfg(feature = "manual-moderation")]
use crate::features::manual_moderation::types::CommandKind as ManualCommandKind;
#[cfg(feature = "voice")]
use crate::features::voice::pipeline::transcribe_reply;
#[cfg(feature = "manual-moderation")]
use crate::telegram::manual_moderation as manual_moderation_frontend;

#[cfg(feature = "ask")]
use super::{handle_ask_command, handle_forget_command, handle_note_command};
#[cfg(feature = "moderation")]
use super::{handle_notspam_command, handle_report_command};

#[cfg(any(feature = "moderation", feature = "manual-moderation"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActorIdentity {
    Human { user_id: i64 },
    Bot { user_id: i64 },
    AnonymousChat { chat_id: i64 },
    Missing,
}

#[cfg(any(feature = "moderation", feature = "manual-moderation"))]
impl ActorIdentity {
    pub(super) fn telegram_user_id(self) -> Option<i64> {
        match self {
            Self::Human { user_id } | Self::Bot { user_id } => Some(user_id),
            Self::AnonymousChat { chat_id } => {
                let _anonymous_chat_id = chat_id;
                None
            }
            Self::Missing => None,
        }
    }
}

#[cfg(feature = "ask")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReplyTarget {
    chat_id: i64,
    message_id: i32,
    user_id: Option<i64>,
}

#[cfg(feature = "ask")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ChatScope {
    chat_id: i64,
}

struct ParsedCommand(Command);

#[cfg(any(
    feature = "ask",
    feature = "moderation",
    feature = "manual-moderation",
    feature = "voice"
))]
struct CommandInvocation<'a> {
    parsed: ParsedCommand,
    bot: &'a teloxide::adaptors::DefaultParseMode<Bot>,
    message: &'a Message,
    state: &'a AppState,
    #[cfg(feature = "ask")]
    scope: ChatScope,
    #[cfg(any(feature = "moderation", feature = "manual-moderation"))]
    actor: ActorIdentity,
    #[cfg(feature = "ask")]
    reply_target: Option<ReplyTarget>,
}

#[cfg(any(
    feature = "ask",
    feature = "moderation",
    feature = "manual-moderation",
    feature = "voice"
))]
impl<'a> CommandInvocation<'a> {
    fn new(
        parsed: ParsedCommand,
        bot: &'a teloxide::adaptors::DefaultParseMode<Bot>,
        message: &'a Message,
        state: &'a AppState,
    ) -> Self {
        #[cfg(any(feature = "moderation", feature = "manual-moderation"))]
        let actor = actor_identity(message);
        #[cfg(feature = "ask")]
        let reply_target = reply_target(message);
        Self {
            parsed,
            bot,
            message,
            state,
            #[cfg(feature = "ask")]
            scope: ChatScope {
                chat_id: message.chat.id.0,
            },
            #[cfg(any(feature = "moderation", feature = "manual-moderation"))]
            actor,
            #[cfg(feature = "ask")]
            reply_target,
        }
    }
}

#[cfg(any(feature = "moderation", feature = "manual-moderation"))]
fn actor_identity(message: &Message) -> ActorIdentity {
    match message.from.as_ref() {
        Some(user) if user.is_bot => ActorIdentity::Bot {
            user_id: user.id.0 as i64,
        },
        Some(user) => ActorIdentity::Human {
            user_id: user.id.0 as i64,
        },
        None => message
            .sender_chat
            .as_ref()
            .map(|chat| ActorIdentity::AnonymousChat { chat_id: chat.id.0 })
            .unwrap_or(ActorIdentity::Missing),
    }
}

#[cfg(feature = "ask")]
fn reply_target(message: &Message) -> Option<ReplyTarget> {
    message.reply_to_message().map(|reply| ReplyTarget {
        chat_id: reply.chat.id.0,
        message_id: reply.id.0,
        user_id: reply.from.as_ref().map(|user| user.id.0 as i64),
    })
}

/// Dispatch only command actions. Chat access, feature gates, and command
/// ingestion stay in the outer handler; authorization remains in each owning
/// service/frontend and is never inferred from menu visibility or chat flags.
pub(super) async fn dispatch(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    message: &Message,
    state: &AppState,
    command: Command,
) -> ResponseResult<Option<Command>> {
    #[cfg(not(any(
        feature = "ask",
        feature = "moderation",
        feature = "manual-moderation",
        feature = "voice"
    )))]
    let _ = (bot, message, state);
    #[cfg(any(
        feature = "ask",
        feature = "moderation",
        feature = "manual-moderation",
        feature = "voice"
    ))]
    let invocation = CommandInvocation::new(ParsedCommand(command), bot, message, state);
    #[cfg(any(
        feature = "ask",
        feature = "moderation",
        feature = "manual-moderation",
        feature = "voice"
    ))]
    let command = invocation.parsed.0;
    #[cfg(not(any(
        feature = "ask",
        feature = "moderation",
        feature = "manual-moderation",
        feature = "voice"
    )))]
    let command = ParsedCommand(command).0;
    match command {
        #[cfg(feature = "voice")]
        Command::Transcribe => {
            transcribe_reply(invocation.bot, invocation.message, invocation.state)
                .await
                .map_err(|err| {
                    tracing::error!(%err, "manual voice transcription command failed");
                    teloxide::RequestError::Io(
                        std::io::Error::other("manual voice transcription failed").into(),
                    )
                })?;
            Ok(None)
        }
        #[cfg(feature = "ask")]
        Command::Ask(question) => {
            handle_ask_command(
                invocation.bot,
                invocation.message,
                invocation.state,
                &question,
            )
            .await?;
            Ok(None)
        }
        #[cfg(feature = "ask")]
        Command::Forget(args) => {
            if args.trim().is_empty() {
                handle_forget_command(invocation.bot, invocation.message, invocation.state).await?;
            }
            Ok(None)
        }
        #[cfg(feature = "ask")]
        Command::ChatNote(note) => {
            handle_note_command(
                invocation.bot,
                invocation.message,
                invocation.state,
                &note,
                None,
            )
            .await?;
            Ok(None)
        }
        #[cfg(feature = "ask")]
        Command::UserNote(note) => {
            handle_note_command(
                invocation.bot,
                invocation.message,
                invocation.state,
                &note,
                invocation.reply_target.and_then(|target| {
                    (target.chat_id == invocation.scope.chat_id && target.message_id > 0)
                        .then_some(target.user_id)
                        .flatten()
                }),
            )
            .await?;
            Ok(None)
        }
        #[cfg(feature = "moderation")]
        Command::Report(reason) => {
            handle_report_command(
                invocation.bot,
                invocation.message,
                invocation.state,
                &reason,
            )
            .await?;
            Ok(None)
        }
        #[cfg(feature = "moderation")]
        Command::Notspam(reason) => {
            handle_notspam_command(
                invocation.bot,
                invocation.message,
                invocation.state,
                &reason,
                invocation.actor,
            )
            .await?;
            Ok(None)
        }
        #[cfg(feature = "manual-moderation")]
        Command::Mute(args) => {
            dispatch_manual_moderation(
                invocation.bot,
                invocation.message,
                invocation.state,
                invocation.actor,
                ManualCommandKind::Mute,
                &args,
            )
            .await
        }
        #[cfg(feature = "manual-moderation")]
        Command::Ban(args) => {
            dispatch_manual_moderation(
                invocation.bot,
                invocation.message,
                invocation.state,
                invocation.actor,
                ManualCommandKind::Ban,
                &args,
            )
            .await
        }
        #[cfg(feature = "manual-moderation")]
        Command::Warn(args) => {
            dispatch_manual_moderation(
                invocation.bot,
                invocation.message,
                invocation.state,
                invocation.actor,
                ManualCommandKind::Warn,
                &args,
            )
            .await
        }
        #[cfg(feature = "manual-moderation")]
        Command::Unmute(args) => {
            dispatch_manual_moderation(
                invocation.bot,
                invocation.message,
                invocation.state,
                invocation.actor,
                ManualCommandKind::Unmute,
                &args,
            )
            .await
        }
        #[cfg(feature = "manual-moderation")]
        Command::Unban(args) => {
            dispatch_manual_moderation(
                invocation.bot,
                invocation.message,
                invocation.state,
                invocation.actor,
                ManualCommandKind::Unban,
                &args,
            )
            .await
        }
        #[cfg(feature = "manual-moderation")]
        Command::Unwarn(args) => {
            dispatch_manual_moderation(
                invocation.bot,
                invocation.message,
                invocation.state,
                invocation.actor,
                ManualCommandKind::Unwarn,
                &args,
            )
            .await
        }
        #[cfg(feature = "manual-moderation")]
        Command::Warns(args) => {
            dispatch_manual_moderation(
                invocation.bot,
                invocation.message,
                invocation.state,
                invocation.actor,
                ManualCommandKind::Warns,
                &args,
            )
            .await
        }
        #[cfg(feature = "manual-moderation")]
        Command::Modlog(args) => {
            dispatch_manual_moderation(
                invocation.bot,
                invocation.message,
                invocation.state,
                invocation.actor,
                ManualCommandKind::Modlog,
                &args,
            )
            .await
        }
        #[cfg(feature = "manual-moderation")]
        Command::Undo => {
            dispatch_manual_moderation(
                invocation.bot,
                invocation.message,
                invocation.state,
                invocation.actor,
                ManualCommandKind::Undo,
                "",
            )
            .await
        }
        #[cfg(feature = "manual-moderation")]
        Command::ChannelOps(args) => {
            crate::telegram::manual_sender_chat::handle_reconcile(
                invocation.bot,
                invocation.message,
                invocation.state,
                &args,
            )
            .await?;
            Ok(None)
        }
        command => Ok(Some(command)),
    }
}

#[cfg(feature = "manual-moderation")]
async fn dispatch_manual_moderation(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    message: &Message,
    state: &AppState,
    actor: ActorIdentity,
    kind: ManualCommandKind,
    args: &str,
) -> ResponseResult<Option<Command>> {
    let _actor = actor;
    manual_moderation_frontend::handle(bot, message, state, kind, args).await?;
    Ok(None)
}

#[cfg(all(test, any(feature = "moderation", feature = "manual-moderation")))]
mod tests {
    use teloxide::types::Message;

    use super::{ActorIdentity, actor_identity, reply_target};

    fn synthetic_message() -> Message {
        serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 1700000000,
            "chat": {"id": -1001000000001_i64, "type": "supergroup"},
            "text": "/action",
            "from": {"id": 42, "is_bot": false, "first_name": "Synthetic"},
            "reply_to_message": null,
            "sender_chat": null
        }))
        .unwrap()
    }

    #[test]
    fn actor_projection_distinguishes_human_bot_and_anonymous_sender() {
        let human = synthetic_message();
        let bot = serde_json::from_value::<Message>(serde_json::json!({
            "message_id": 1, "date": 1700000000,
            "chat": {"id": -1001000000001_i64, "type": "supergroup"},
            "text": "/action",
            "from": {"id": 43, "is_bot": true, "first_name": "Synthetic bot"}
        }))
        .unwrap();
        let anonymous = serde_json::from_value::<Message>(serde_json::json!({
            "message_id": 1, "date": 1700000000,
            "chat": {"id": -1001000000001_i64, "type": "supergroup"},
            "text": "/action",
            "sender_chat": {"id": -1001000000001_i64, "type": "supergroup", "title": "Anonymous"}
        }))
        .unwrap();
        let missing = serde_json::from_value::<Message>(serde_json::json!({
            "message_id": 1, "date": 1700000000,
            "chat": {"id": -1001000000001_i64, "type": "supergroup"},
            "text": "/action"
        }))
        .unwrap();
        assert_eq!(actor_identity(&human), ActorIdentity::Human { user_id: 42 });
        assert_eq!(actor_identity(&bot), ActorIdentity::Bot { user_id: 43 });
        assert_eq!(
            actor_identity(&anonymous),
            ActorIdentity::AnonymousChat {
                chat_id: -1001000000001
            }
        );
        assert_eq!(actor_identity(&missing), ActorIdentity::Missing);
    }

    #[test]
    fn reply_projection_keeps_only_same_chat_positive_targets() {
        let same_chat = serde_json::from_value::<Message>(serde_json::json!({
            "message_id": 2, "date": 1700000001,
            "chat": {"id": -1001000000001_i64, "type": "supergroup"},
            "text": "/usernote",
            "reply_to_message": {
                "message_id": 1, "date": 1700000000,
                "chat": {"id": -1001000000001_i64, "type": "supergroup"},
                "text": "synthetic",
                "from": {"id": 52, "is_bot": false, "first_name": "Target"}
            }
        }))
        .unwrap();
        let target = reply_target(&same_chat).expect("reply target should be projected");
        assert_eq!(target.chat_id, -1001000000001);
        assert_eq!(target.message_id, 1);
        assert_eq!(target.user_id, Some(52));

        let malformed_or_cross_chat = serde_json::from_value::<Message>(serde_json::json!({
            "message_id": 2, "date": 1700000001,
            "chat": {"id": -1001000000001_i64, "type": "supergroup"},
            "text": "/usernote",
            "reply_to_message": {
                "message_id": 0, "date": 1700000000,
                "chat": {"id": -1002000000002_i64, "type": "supergroup"},
                "text": "synthetic",
                "from": {"id": 52, "is_bot": false, "first_name": "Target"}
            }
        }))
        .unwrap();
        let target = reply_target(&malformed_or_cross_chat).unwrap();
        assert_ne!(target.chat_id, -1001000000001);
        assert_eq!(target.message_id, 0);
    }
}
