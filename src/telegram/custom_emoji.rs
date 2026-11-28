use teloxide::prelude::*;

use crate::telegram::entities::{CustomEmojiOccurrence, custom_emoji_occurrences};
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
    let occurrences = custom_emoji_occurrences(msg);
    if occurrences.is_empty() {
        service_messages::send_html(
            bot,
            msg.chat.id,
            "В этом сообщении нет premium/custom emoji entities.",
            audience,
        )
        .await?;
        return Ok(());
    }

    let lines = occurrences
        .iter()
        .map(render_occurrence)
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

fn render_occurrence(occurrence: &CustomEmojiOccurrence) -> String {
    let id = escape_html(&occurrence.custom_emoji_id);
    match occurrence.fallback.as_deref() {
        Some(fallback) if !fallback.is_empty() => {
            format!("{} → <code>{}</code>", escape_html(fallback), id)
        }
        _ => format!("<code>{}</code>", id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_fallback_per_occurrence_and_escapes_html() {
        let rows = [
            CustomEmojiOccurrence {
                custom_emoji_id: "123".to_owned(),
                fallback: Some("<AMD>".to_owned()),
            },
            CustomEmojiOccurrence {
                custom_emoji_id: "123".to_owned(),
                fallback: Some("🙂".to_owned()),
            },
            CustomEmojiOccurrence {
                custom_emoji_id: "456".to_owned(),
                fallback: None,
            },
        ];
        assert_eq!(
            rows.iter().map(render_occurrence).collect::<Vec<_>>(),
            [
                "&lt;AMD&gt; → <code>123</code>",
                "🙂 → <code>123</code>",
                "<code>456</code>"
            ]
        );
    }
}
