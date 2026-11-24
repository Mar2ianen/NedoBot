//! Fresh authorization checks for Telegram effects, separate from startup
//! observations. A successful check is not a lease: Telegram can revoke rights
//! before the subsequent request, whose error must still be handled.

use teloxide::{
    prelude::*,
    types::{ChatId, ChatMemberKind},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequiredChatRight {
    DeleteMessages,
    RestrictMembers,
    DeleteAndRestrict,
}

pub fn allows_chat_right(member: &ChatMemberKind, right: RequiredChatRight) -> bool {
    match right {
        RequiredChatRight::DeleteMessages => member.can_delete_messages(),
        RequiredChatRight::RestrictMembers => member.can_restrict_members(),
        RequiredChatRight::DeleteAndRestrict => {
            member.can_delete_messages() && member.can_restrict_members()
        }
    }
}

pub async fn require_chat_right(
    bot: &Bot,
    chat_id: ChatId,
    right: RequiredChatRight,
) -> anyhow::Result<()> {
    let me = bot
        .get_me()
        .await
        .map_err(|_| anyhow::anyhow!("cannot verify bot identity"))?;
    let member = bot
        .get_chat_member(chat_id, me.id)
        .await
        .map_err(|_| anyhow::anyhow!("cannot verify bot membership in chat {}", chat_id.0))?;
    anyhow::ensure!(
        member.user.id == me.id && allows_chat_right(&member.kind, right),
        "required Telegram right {right:?} is unavailable in chat {}",
        chat_id.0
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, http::Uri, routing::post};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use teloxide::types::ChatMember;

    #[test]
    fn required_rights_do_not_infer_restriction_from_deletion_or_admin_status() {
        for (delete, restrict, expected) in [
            (false, false, [false, false, false]),
            (true, false, [true, false, false]),
            (false, true, [false, true, false]),
            (true, true, [true, true, true]),
        ] {
            let member: ChatMember = serde_json::from_value(serde_json::json!({
                "status": "administrator",
                "user": {"id": 9001, "is_bot": true, "first_name": "Synthetic"},
                "can_be_edited": false, "is_anonymous": false,
                "can_manage_chat": true, "can_delete_messages": delete,
                "can_manage_video_chats": false, "can_restrict_members": restrict,
                "can_promote_members": false, "can_change_info": false,
                "can_invite_users": false, "can_post_stories": false,
                "can_edit_stories": false, "can_delete_stories": false,
                "can_post_messages": false, "can_edit_messages": false,
                "can_pin_messages": false, "can_manage_topics": false
            }))
            .unwrap();
            for (right, expected) in [
                RequiredChatRight::DeleteMessages,
                RequiredChatRight::RestrictMembers,
                RequiredChatRight::DeleteAndRestrict,
            ]
            .into_iter()
            .zip(expected)
            {
                assert_eq!(allows_chat_right(&member.kind, right), expected);
            }
        }
        let member: ChatMember = serde_json::from_value(serde_json::json!({
            "status": "left",
            "user": {"id": 9001, "is_bot": true, "first_name": "Synthetic"}
        }))
        .unwrap();
        assert!(!allows_chat_right(
            &member.kind,
            RequiredChatRight::DeleteAndRestrict
        ));
    }

    #[tokio::test]
    async fn runtime_right_check_queries_fresh_membership_and_fails_closed_on_identity_mismatch() {
        #[derive(Default)]
        struct Stub {
            calls: Vec<String>,
            member: Value,
        }

        async fn api(
            State(stub): State<Arc<Mutex<Stub>>>,
            uri: Uri,
            Json(payload): Json<Value>,
        ) -> Json<Value> {
            let method = uri.path().rsplit('/').next().unwrap().to_ascii_lowercase();
            let mut stub = stub.lock().unwrap();
            stub.calls.push(method.clone());
            match method.as_str() {
                "getme" => Json(json!({"ok": true, "result": {
                    "id": 9001, "is_bot": true, "first_name": "Synthetic",
                    "can_join_groups": true, "can_read_all_group_messages": true,
                    "supports_inline_queries": false, "needs_inline_query_location": false,
                    "can_connect_to_business": false, "supports_guest_queries": false,
                    "supports_join_request_queries": false, "has_topics_enabled": false,
                    "allows_users_to_create_topics": false, "has_topic_sidebar": false,
                    "can_manage_bots": false, "has_main_web_app": false
                }})),
                "getchatmember" => {
                    assert_eq!(payload["chat_id"], -10042);
                    assert_eq!(payload["user_id"], 9001);
                    Json(json!({"ok": true, "result": stub.member.clone()}))
                }
                _ => panic!("unexpected API method: {method}"),
            }
        }

        let member = |user_id, delete, restrict| {
            json!({
                "status": "administrator",
                "user": {"id": user_id, "is_bot": true, "first_name": "Synthetic"},
                "can_be_edited": false, "is_anonymous": false,
                "can_manage_chat": true, "can_delete_messages": delete,
                "can_manage_video_chats": false, "can_restrict_members": restrict,
                "can_promote_members": false, "can_change_info": false,
                "can_invite_users": false, "can_post_stories": false,
                "can_edit_stories": false, "can_delete_stories": false,
                "can_post_messages": false, "can_edit_messages": false,
                "can_pin_messages": false, "can_manage_topics": false
            })
        };
        let stub = Arc::new(Mutex::new(Stub {
            calls: Vec::new(),
            member: member(9001, true, false),
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new().fallback(post(api)).with_state(stub.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let bot =
            Bot::new("fixture-token").set_api_url(format!("http://{address}/").parse().unwrap());
        let chat_id = ChatId(-10042);

        assert!(
            require_chat_right(&bot, chat_id, RequiredChatRight::DeleteMessages)
                .await
                .is_ok()
        );
        assert!(
            require_chat_right(&bot, chat_id, RequiredChatRight::RestrictMembers)
                .await
                .is_err()
        );
        stub.lock().unwrap().member = member(9002, true, true);
        assert!(
            require_chat_right(&bot, chat_id, RequiredChatRight::DeleteAndRestrict)
                .await
                .is_err()
        );
        assert_eq!(
            stub.lock().unwrap().calls,
            [
                "getme",
                "getchatmember",
                "getme",
                "getchatmember",
                "getme",
                "getchatmember"
            ]
        );
        server.abort();
    }
}
