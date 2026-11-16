//! Telegram update routing and its precedence rules.
//!
//! This module owns only the dptree graph. Endpoint functions stay with their
//! application effects in `main` and feature adapters; dependencies are still
//! injected by `Dispatcher::builder` at the root.

use teloxide::{RequestError, dispatching::UpdateHandler};

mod callbacks;
mod commands;
mod members;
mod messages;
mod reactions;

pub type Handler = UpdateHandler<RequestError>;

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
    members::append(handler, routes.chat_member)
}

#[cfg(test)]
mod tests {
    use std::ops::ControlFlow;
    use std::sync::{Arc, Mutex};

    use teloxide::{
        dispatching::HandlerExt,
        dptree,
        prelude::{Message, ResponseResult},
        types::{Me, Update},
    };

    use crate::telegram::commands::Command;

    use super::{Handler, commands};

    #[derive(Clone)]
    struct RouteCalls(Arc<Mutex<Vec<&'static str>>>);

    async fn record_command(
        _message: Message,
        _command: Command,
        calls: RouteCalls,
    ) -> ResponseResult<()> {
        calls.0.lock().unwrap().push("command");
        Ok(())
    }

    async fn record_message(_message: Message, calls: RouteCalls) -> ResponseResult<()> {
        calls.0.lock().unwrap().push("message");
        Ok(())
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
            "has_main_web_app": false
        }))
        .expect("synthetic bot identity must match the Telegram Me schema")
    }

    fn test_update(text: &str) -> Update {
        serde_json::from_value(serde_json::json!({
            "update_id": 1001,
            "message": {
                "message_id": 1001,
                "date": 1700000000,
                "chat": {"id": -1001000000001_i64, "type": "supergroup"},
                "from": {"id": 900000002, "is_bot": false, "first_name": "Synthetic user"},
                "text": text
            }
        }))
        .expect("synthetic update must deserialize using Telegram's typed model")
    }

    fn production_message_route() -> Handler {
        commands::command_first(
            dptree::entry()
                .filter_command::<Command>()
                .endpoint(record_command),
            dptree::endpoint(record_message),
        )
    }

    #[tokio::test]
    async fn production_message_route_dispatches_commands_before_ordinary_messages() {
        let calls = RouteCalls(Arc::new(Mutex::new(Vec::new())));
        let outcome = production_message_route()
            .dispatch(dptree::deps![
                test_update("/ping"),
                test_me(),
                calls.clone()
            ])
            .await;

        assert!(matches!(outcome, ControlFlow::Break(Ok(()))));
        assert_eq!(*calls.0.lock().unwrap(), ["command"]);
    }

    #[tokio::test]
    async fn production_message_route_sends_non_commands_to_ingress() {
        let calls = RouteCalls(Arc::new(Mutex::new(Vec::new())));
        let outcome = production_message_route()
            .dispatch(dptree::deps![
                test_update("ordinary synthetic message"),
                test_me(),
                calls.clone()
            ])
            .await;

        assert!(matches!(outcome, ControlFlow::Break(Ok(()))));
        assert_eq!(*calls.0.lock().unwrap(), ["message"]);
    }
}
