//! Telegram update routing and its precedence rules.
//!
//! This module owns only the dptree graph. Endpoint functions stay with their
//! application effects in `main` and feature adapters; dependencies are still
//! injected by `Dispatcher::builder` at the root.

use teloxide::{RequestError, dispatching::UpdateHandler};

#[cfg(feature = "moderation")]
mod callbacks;
mod commands;
mod members;
mod messages;
mod reactions;

pub type Handler = UpdateHandler<RequestError>;

/// Parse commands using only names returned for the authenticated bot. An
/// absent username does not prevent unaddressed commands or panic the router.
pub fn parse_bot_command(
    message: teloxide::types::Message,
    me: teloxide::types::Me,
) -> Option<super::commands::Command> {
    use teloxide::utils::command::BotCommands;

    let text = message.text().or_else(|| message.caption())?;
    let word = text.split_whitespace().next()?;
    let username = match word.split_once('@') {
        None => "",
        Some((_, addressed)) => {
            if addressed.is_empty()
                || !me
                    .username_opt()
                    .into_iter()
                    .chain(me.active_usernames.iter().flatten().map(String::as_str))
                    .any(|name| name.eq_ignore_ascii_case(addressed))
            {
                return None;
            }
            addressed
        }
    };
    super::commands::Command::parse(text, username).ok()
}

/// Fully typed endpoint set for the dispatcher tree.
pub struct Routes {
    pub command: Handler,
    pub message: Handler,
    pub reaction: Handler,
    pub reaction_count: Handler,
    pub edited_message: Handler,
    #[cfg(feature = "moderation")]
    pub callback: Handler,
    pub chat_member: Handler,
    pub my_chat_member: Handler,
}

/// Build the production update graph. Keep this order aligned with the route
/// characterization tests: message command before ordinary message, reactions
/// before edits, optional callbacks after message updates, then chat members.
pub fn build(routes: Routes) -> Handler {
    let handler = commands::command_first(routes.command, routes.message);
    let handler = reactions::append(handler, routes.reaction, routes.reaction_count);
    let handler = messages::append_edited(handler, routes.edited_message);
    #[cfg(feature = "moderation")]
    let handler = callbacks::append(handler, routes.callback);
    members::append(handler, routes.chat_member).branch(routes.my_chat_member)
}

#[cfg(test)]
mod tests {
    use std::ops::ControlFlow;
    use std::sync::{Arc, Mutex};

    #[cfg(feature = "moderation")]
    use teloxide::types::CallbackQuery;
    use teloxide::{
        dispatching::UpdateFilterExt,
        dptree,
        prelude::{Message, ResponseResult},
        types::{
            ChatMemberUpdated, Me, MessageReactionCountUpdated, MessageReactionUpdated, Update,
        },
    };

    use crate::telegram::commands::Command;

    use super::{Handler, Routes, build, parse_bot_command};

    #[derive(Clone)]
    struct RouteCalls(Arc<Mutex<Vec<&'static str>>>);

    fn record(calls: &RouteCalls, name: &'static str) {
        calls
            .0
            .lock()
            .expect("route-call mutex must not be poisoned")
            .push(name);
    }

    async fn command(
        _message: Message,
        _command: Command,
        calls: RouteCalls,
    ) -> ResponseResult<()> {
        record(&calls, "command");
        Ok(())
    }

    async fn message(_message: Message, calls: RouteCalls) -> ResponseResult<()> {
        record(&calls, "message");
        Ok(())
    }

    async fn edited(_message: Message, calls: RouteCalls) -> ResponseResult<()> {
        record(&calls, "edited");
        Ok(())
    }

    async fn reaction(_update: MessageReactionUpdated, calls: RouteCalls) -> ResponseResult<()> {
        record(&calls, "reaction");
        Ok(())
    }

    async fn reaction_count(
        _update: MessageReactionCountUpdated,
        calls: RouteCalls,
    ) -> ResponseResult<()> {
        record(&calls, "reaction_count");
        Ok(())
    }

    #[cfg(feature = "moderation")]
    async fn callback(_query: CallbackQuery, calls: RouteCalls) -> ResponseResult<()> {
        record(&calls, "callback");
        Ok(())
    }

    async fn chat_member(_update: ChatMemberUpdated, calls: RouteCalls) -> ResponseResult<()> {
        record(&calls, "chat_member");
        Ok(())
    }

    async fn my_chat_member(_update: ChatMemberUpdated, calls: RouteCalls) -> ResponseResult<()> {
        record(&calls, "my_chat_member");
        Ok(())
    }

    fn production_handler() -> Handler {
        build(Routes {
            command: dptree::filter_map(parse_bot_command).endpoint(command),
            message: dptree::endpoint(message),
            reaction: Update::filter_message_reaction_updated().endpoint(reaction),
            reaction_count: Update::filter_message_reaction_count_updated()
                .endpoint(reaction_count),
            edited_message: Update::filter_edited_message().endpoint(edited),
            #[cfg(feature = "moderation")]
            callback: Update::filter_callback_query().endpoint(callback),
            chat_member: Update::filter_chat_member().endpoint(chat_member),
            my_chat_member: Update::filter_my_chat_member().endpoint(my_chat_member),
        })
    }

    fn test_me() -> Me {
        serde_json::from_value(serde_json::json!({
            "id": 900000001,
            "is_bot": true,
            "first_name": "Synthetic bot",
            "username": "nedobot",
            "can_join_groups": true,
            "can_read_all_group_messages": true,
            "supports_inline_queries": false,
            "needs_inline_query_location": false,
            "can_connect_to_business": false,
            "supports_guest_queries": false,
            "supports_join_request_queries": false,
            "has_topics_enabled": false,
            "allows_users_to_create_topics": false,
            "has_topic_sidebar": false,
            "can_manage_bots": false,
            "has_main_web_app": false
        }))
        .expect("synthetic bot identity must match the Telegram Me schema")
    }

    fn message_value(text: &str) -> serde_json::Value {
        serde_json::json!({
            "message_id": 1001,
            "date": 1700000000,
            "chat": {"id": -1001000000001_i64, "type": "supergroup"},
            "from": {"id": 900000002, "is_bot": false, "first_name": "Synthetic user"},
            "text": text
        })
    }

    fn update(field: &str, payload: serde_json::Value) -> Update {
        let mut object = serde_json::json!({"update_id": 1001});
        object[field] = payload;
        serde_json::from_value(object)
            .expect("synthetic update must deserialize as Telegram Update")
    }

    async fn dispatch(
        update: Update,
        calls: &RouteCalls,
    ) -> ControlFlow<Result<(), teloxide::RequestError>, dptree::di::DependencyMap> {
        production_handler()
            .dispatch(dptree::deps![update, test_me(), calls.clone()])
            .await
    }

    #[test]
    fn addressed_commands_accept_active_aliases_without_requiring_primary_username() {
        let mut me = test_me();
        me.user.username = None;
        me.active_usernames = Some(vec!["Alias_Bot".to_owned()]);
        for (text, accepted) in [
            ("/ping", true),
            ("/ping@alias_bot", true),
            ("/ping@Other_Bot", false),
            ("/ping@", false),
        ] {
            let message = serde_json::from_value(message_value(text)).unwrap();
            assert_eq!(parse_bot_command(message, me.clone()).is_some(), accepted);
        }
    }

    #[tokio::test]
    async fn actual_router_builder_prefers_command_and_falls_back_for_unknown_command() {
        let calls = RouteCalls(Arc::new(Mutex::new(Vec::new())));
        let outcome = dispatch(update("message", message_value("/ping")), &calls).await;
        assert!(matches!(outcome, ControlFlow::Break(Ok(()))));
        assert_eq!(*calls.0.lock().unwrap(), ["command"]);

        calls.0.lock().unwrap().clear();
        let outcome = dispatch(update("message", message_value("/not-a-command")), &calls).await;
        assert!(matches!(outcome, ControlFlow::Break(Ok(()))));
        assert_eq!(*calls.0.lock().unwrap(), ["message"]);
    }

    #[tokio::test]
    async fn actual_router_builder_routes_edits_separately_from_new_messages() {
        let calls = RouteCalls(Arc::new(Mutex::new(Vec::new())));
        let outcome = dispatch(
            update("edited_message", message_value("edited synthetic message")),
            &calls,
        )
        .await;
        assert!(matches!(outcome, ControlFlow::Break(Ok(()))));
        assert_eq!(*calls.0.lock().unwrap(), ["edited"]);
    }

    #[tokio::test]
    async fn actual_router_builder_routes_reaction_update_count_and_chat_member_events() {
        let calls = RouteCalls(Arc::new(Mutex::new(Vec::new())));
        let chat = serde_json::json!({"id": -1001000000001_i64, "type": "supergroup"});
        let user =
            serde_json::json!({"id": 900000002, "is_bot": false, "first_name": "Synthetic user"});

        for (field, payload, expected) in [
            (
                "message_reaction",
                serde_json::json!({
                    "chat": chat, "message_id": 1001, "user": user, "date": 1700000000,
                    "old_reaction": [], "new_reaction": [{"type":"emoji", "emoji":"👍"}]
                }),
                "reaction",
            ),
            (
                "message_reaction_count",
                serde_json::json!({
                    "chat": chat, "message_id": 1001, "date": 1700000000,
                    "reactions": [{"type":{"type":"emoji", "emoji":"👍"}, "total_count": 2}]
                }),
                "reaction_count",
            ),
            (
                "chat_member",
                serde_json::json!({
                    "chat": chat, "from": user, "date": 1700000000,
                    "old_chat_member": {"status":"member", "user":user},
                    "new_chat_member": {"status":"administrator", "user":user, "can_be_edited":false,
                        "is_anonymous":false, "can_manage_chat":true, "can_delete_messages":false,
                        "can_manage_video_chats":false, "can_restrict_members":false,
                        "can_promote_members":false, "can_change_info":false,
                        "can_invite_users":false, "can_post_stories":false,
                        "can_edit_stories":false, "can_delete_stories":false,
                        "can_post_messages":false, "can_edit_messages":false,
                        "can_pin_messages":false, "can_manage_topics":false}
                }),
                "chat_member",
            ),
        ] {
            calls.0.lock().unwrap().clear();
            let telegram_update = update(field, payload);
            if field == "message_reaction_count" {
                assert!(matches!(
                    telegram_update.kind,
                    teloxide::types::UpdateKind::MessageReactionCount(_)
                ));
            }
            let outcome = dispatch(telegram_update, &calls).await;
            assert!(
                matches!(outcome, ControlFlow::Break(Ok(()))),
                "route={field} calls={:?} result={outcome:?}",
                *calls.0.lock().unwrap()
            );
            assert_eq!(*calls.0.lock().unwrap(), [expected], "route={field}");
        }
    }

    #[cfg(feature = "moderation")]
    #[tokio::test]
    async fn actual_router_builder_routes_callback_updates_once() {
        let calls = RouteCalls(Arc::new(Mutex::new(Vec::new())));
        let query = serde_json::json!({
            "id":"synthetic-callback-id",
            "from":{"id":900000002,"is_bot":false,"first_name":"Synthetic user"},
            "chat_instance":"synthetic-chat-instance",
            "data":"unsupported:synthetic"
        });
        let outcome = dispatch(update("callback_query", query), &calls).await;
        assert!(matches!(outcome, ControlFlow::Break(Ok(()))));
        assert_eq!(*calls.0.lock().unwrap(), ["callback"]);
    }

    #[tokio::test]
    async fn actual_router_builder_routes_bot_membership_separately() {
        let calls = RouteCalls(Arc::new(Mutex::new(Vec::new())));
        let membership = update(
            "my_chat_member",
            serde_json::json!({
                "chat":{"id":-1001000000001_i64,"type":"supergroup"},
                "from":{"id":900000002,"is_bot":false,"first_name":"Synthetic user"},
                "date":1700000000,
                "old_chat_member":{"status":"left","user":{"id":900000001,"is_bot":true,"first_name":"Synthetic bot"}},
                "new_chat_member":{"status":"member","user":{"id":900000001,"is_bot":true,"first_name":"Synthetic bot"}}
            }),
        );
        let outcome = dispatch(membership, &calls).await;
        assert!(matches!(outcome, ControlFlow::Break(Ok(()))));
        assert_eq!(*calls.0.lock().unwrap(), ["my_chat_member"]);
    }

    #[tokio::test]
    async fn actual_router_builder_has_no_side_effects_for_unsupported_updates() {
        let calls = RouteCalls(Arc::new(Mutex::new(Vec::new())));
        let unsupported = update(
            "inline_query",
            serde_json::json!({
                "id": "synthetic-query",
                "from": {"id": 900000002, "is_bot": false, "first_name": "Synthetic user"},
                "query": "unsupported",
                "offset": ""
            }),
        );
        let outcome = dispatch(unsupported, &calls).await;
        assert!(matches!(outcome, ControlFlow::Continue(_)));
        assert!(calls.0.lock().unwrap().is_empty());
    }
}
