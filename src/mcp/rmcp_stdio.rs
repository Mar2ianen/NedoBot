//! RMCP stdio adapter for the reviewed public chat read-model.

use anyhow::Result;
use rmcp::ServiceExt;

use super::bootstrap::{RmcpStdioConfig, build_chat_mcp_server};

/// Serves the official RMCP lifecycle on stdin/stdout.
///
/// Configuration is intentionally inherited only from the parent environment;
/// standalone child processes must receive both required variables explicitly.
pub async fn run_stdio_server() -> Result<()> {
    let config = RmcpStdioConfig::from_env()?;
    build_chat_mcp_server(config)
        .await?
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}
