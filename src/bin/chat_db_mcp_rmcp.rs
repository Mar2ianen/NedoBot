#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tg_ai_bot_teloxide::mcp::rmcp_stdio::run_stdio_server().await
}
