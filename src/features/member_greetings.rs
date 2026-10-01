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

    let joined =
        !update.old_chat_member.kind.is_present() && update.new_chat_member.kind.is_present();
    let left =
        update.old_chat_member.kind.is_present() && !update.new_chat_member.kind.is_present();
    if !joined && !left {
        return Ok(());
    }

    let user = &update.new_chat_member.user;
    if user.is_bot {
        return Ok(());
    }
    let template = if joined {
        chat.config.welcome_message.as_deref()
    } else {
        chat.config.farewell_message.as_deref()
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
    template
        .replace("{first_name}", first_name)
        .replace("{last_name}", last_name)
        .replace("{full_name}", full_name)
        .replace("{username}", username)
        .replace("{user_id}", user_id)
        .replace("{chat_title}", chat_title)
}
