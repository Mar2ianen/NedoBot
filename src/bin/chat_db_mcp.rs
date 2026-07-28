#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Child получает только явный allowlist `McpClient` после `env_clear()`.
    tg_ai_bot_teloxide::mcp::rmcp_stdio::run_stdio_server().await
}
