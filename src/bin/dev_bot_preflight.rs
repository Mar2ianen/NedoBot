use anyhow::Context;
use teloxide::{
    Bot,
    prelude::{Request, Requester},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::from_filename(".env.dev")
        .context("failed to load local .env.dev for Telegram dev-bot preflight")?;
    let bot = Bot::from_env();
    let bot_user = bot.get_me().send().await.context("Telegram getMe failed")?;
    println!(
        "Telegram bot configured by .env.dev is reachable: @{} (id={})",
        bot_user
            .user
            .username
            .as_deref()
            .unwrap_or("without_username"),
        bot_user.user.id
    );
    Ok(())
}
