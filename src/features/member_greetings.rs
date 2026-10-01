use teloxide::{
    adaptors::DefaultParseMode,
    prelude::{Bot, ChatMemberUpdated, ResponseResult},
};

use crate::{
    config::Config,
    telegram::{
        render::escape_html,
        service_messages::{self, MessageAudience},
    },
};

pub async fn handle_chat_member_update(
    bot: &DefaultParseMode<Bot>,
    update: &ChatMemberUpdated,
    config: &Config,
) -> ResponseResult<()> {
    let Some(chat) = config.chat_by_id(update.chat.id.0) else {
        return Ok(());
    };

    let Some(transition) = membership_transition(
        update.old_chat_member.kind.is_present(),
        update.new_chat_member.kind.is_present(),
    ) else {
        return Ok(());
    };

    let user = &update.new_chat_member.user;
    if user.is_bot {
        return Ok(());
    }
    let template = match transition {
        MembershipTransition::Joined => chat.config.welcome_message.as_deref(),
        MembershipTransition::Left => chat.config.farewell_message.as_deref(),
    };
    let Some(template) = template.filter(|template| !template.trim().is_empty()) else {
        return Ok(());
    };

    let text = render_template(
        template,
        &escape_html(&user.first_name),
        &escape_html(user.last_name.as_deref().unwrap_or_default()),
        &escape_html(&user.full_name()),
        &escape_html(user.username.as_deref().unwrap_or_default()),
        &user.id.0.to_string(),
        &escape_html(update.chat.title().unwrap_or_default()),
    );
    service_messages::send_html(bot, update.chat.id, text, MessageAudience::Public).await?;
    Ok(())
}

fn render_template(
    template: &str,
    first_name: &str,
    last_name: &str,
    full_name: &str,
    username: &str,
    user_id: &str,
    chat_title: &str,
) -> String {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(open_brace) = rest.find('{') {
        rendered.push_str(&rest[..open_brace]);
        let placeholder_start = open_brace + 1;
        let Some(close_offset) = rest[placeholder_start..].find('}') else {
            rendered.push_str(&rest[open_brace..]);
            return rendered;
        };
        let close_brace = placeholder_start + close_offset;
        let placeholder = &rest[placeholder_start..close_brace];
        let value = match placeholder {
            "first_name" => Some(first_name),
            "last_name" => Some(last_name),
            "full_name" => Some(full_name),
            "username" => Some(username),
            "user_id" => Some(user_id),
            "chat_title" => Some(chat_title),
            _ => None,
        };
        if let Some(value) = value {
            rendered.push_str(value);
        } else {
            rendered.push_str(&rest[open_brace..=close_brace]);
        }
        rest = &rest[close_brace + 1..];
    }
    rendered.push_str(rest);
    rendered
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MembershipTransition {
    Joined,
    Left,
}

fn membership_transition(was_present: bool, is_present: bool) -> Option<MembershipTransition> {
    match (was_present, is_present) {
        (false, true) => Some(MembershipTransition::Joined),
        (true, false) => Some(MembershipTransition::Left),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{MembershipTransition, membership_transition, render_template};

    #[test]
    fn membership_transition_matrix_only_reports_join_and_leave_edges() {
        assert_eq!(
            membership_transition(false, false),
            None,
            "absent to absent is not a transition"
        );
        assert_eq!(
            membership_transition(false, true),
            Some(MembershipTransition::Joined)
        );
        assert_eq!(
            membership_transition(true, false),
            Some(MembershipTransition::Left)
        );
        assert_eq!(membership_transition(true, true), None);
    }

    #[test]
    fn inserted_values_are_not_scanned_again_for_placeholders() {
        let rendered = render_template(
            "{first_name} joined {chat_title}",
            "{chat_title}",
            "",
            "{chat_title}",
            "",
            "42",
            "Forum",
        );

        assert_eq!(rendered, "{chat_title} joined Forum");
    }

    #[test]
    fn unknown_placeholders_remain_unchanged() {
        assert_eq!(
            render_template("{unknown}: {user_id}", "", "", "", "", "42", ""),
            "{unknown}: 42"
        );
    }
}
