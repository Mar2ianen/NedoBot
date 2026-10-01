use teloxide::prelude::*;

use crate::telegram::entities::custom_emoji_ids;
use crate::telegram::{
    render::escape_html,
    service_messages::{self, MessageAudience},
};

pub async fn send_custom_emoji_ids(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    audience: Option<MessageAudience>,
) -> ResponseResult<()> {
    let Some(audience) = audience else {
        return Ok(());
    };
    let ids = custom_emoji_ids(msg);
    if ids.is_empty() {
        service_messages::send_html(
            bot,
            msg.chat.id,
            "В этом сообщении нет premium/custom emoji entities.",
            audience,
        )
        .await?;
        return Ok(());
    }

    let lines = ids
        .iter()
        .map(|id| format!("<code>{}</code>", escape_html(id)))
        .collect::<Vec<_>>()
        .join("\n");

    service_messages::send_html(
        bot,
        msg.chat.id,
        format!("Нашёл custom_emoji_id:\n{}", lines),
        audience,
    )
    .await?;

    Ok(())
}
