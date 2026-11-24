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
}
