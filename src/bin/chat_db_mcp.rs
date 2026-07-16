#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tg_ai_bot_teloxide::features::ask::chat_db_mcp::run_stdio_server().await
}
