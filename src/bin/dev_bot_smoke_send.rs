use anyhow::Context;
use teloxide::{
    Bot,
    prelude::{Request, Requester},
    types::ChatId,
};
use tg_ai_bot_teloxide::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::from_filename(".env.dev")
        .context("failed to load local .env.dev for Telegram dev-bot smoke send")?;
    let config = Config::from_env()?;
    let bot = Bot::from_env();
    let message = bot
        .send_message(
            ChatId(config.discussion_chat_id),
            "✅ dev bot smoke test: Telegram API send succeeded.",
        )
        .send()
        .await
        .context("Telegram dev-bot smoke send failed")?;
    println!(
        "Telegram dev-bot smoke message sent to chat_id={} (message_id={})",
        config.discussion_chat_id, message.id
    );
    Ok(())
}
