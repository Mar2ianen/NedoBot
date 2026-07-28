//! RMCP-08 canary HTTP entry point. The legacy `nedonews_mcp_http` stays unchanged.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tg_ai_bot_teloxide::mcp::rmcp_http::run_public_http().await
}
