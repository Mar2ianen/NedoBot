use teloxide::prelude::*;

use crate::telegram::entities::custom_emoji_ids;
use crate::telegram::render::{escape_html, send_html};

pub async fn send_custom_emoji_ids(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
) -> ResponseResult<()> {
    let ids = custom_emoji_ids(msg);
    if ids.is_empty() {
        send_html(
            bot,
            msg.chat.id,
            "В этом сообщении нет premium/custom emoji entities.",
        )
        .await?;
        return Ok(());
    }

    let lines = ids
        .iter()
        .map(|id| format!("<code>{}</code>", escape_html(id)))
        .collect::<Vec<_>>()
        .join("\n");

    send_html(
        bot,
        msg.chat.id,
        format!("Нашёл custom_emoji_id:\n{}", lines),
    )
    .await?;

    Ok(())
}
