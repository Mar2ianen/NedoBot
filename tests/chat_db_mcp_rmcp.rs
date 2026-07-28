use std::{future::Future, path::PathBuf, time::Duration};

use anyhow::Result;
use tg_ai_bot_teloxide::{config::Config, features::ask::mcp_client::McpClient};

const E2E_TIMEOUT: Duration = Duration::from_secs(10);

async fn within_timeout<T>(scenario: &str, future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(E2E_TIMEOUT, future)
        .await
        .map_err(|_| anyhow::anyhow!("{scenario} exceeded {E2E_TIMEOUT:?}"))?
}

fn manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/mcp_db_manifest.toml")
}

#[ignore = "requires TEST_DATABASE_URL and a migrated local PostgreSQL database"]
#[tokio::test]
async fn ask_mcp_client_starts_real_rmcp_child_with_env_clear_allowlist() -> Result<()> {
    within_timeout("McpClient real RMCP child lifecycle", async {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must be set by scripts/test.sh");
        let manifest = manifest_path();

        // McpClient clears the child environment. These are the only two inherited
        // settings the real stdio server needs, so this also catches allowlist drift.
        unsafe {
            std::env::set_var("ASK_DATABASE_URL", &database_url);
            std::env::set_var("MCP_MANIFEST", &manifest);
        }
        let mut config = Config::from_env()?;
        config.ask_db_mcp_command = Some(env!("CARGO_BIN_EXE_chat_db_mcp_rmcp").to_string());
        config.ask_db_mcp_args = Vec::new();
        config.ask_db_mcp_env = vec!["ASK_DATABASE_URL".to_string(), "MCP_MANIFEST".to_string()];
        config.ask_db_mcp_timeout_sec = E2E_TIMEOUT.as_secs();

        let client = McpClient::start(&config).await?;
        assert!(client.has_tool("chat.resolve_user"));
        assert!(!client.has_tool("db.list_tables"));

        let result = client
            .call(
                "chat.resolve_user",
                serde_json::json!({"query": "unlikely-user"}),
            )
            .await?;
        assert!(result.value.is_object());
        assert!(serde_json::from_str::<serde_json::Value>(&result.agent_preview).is_ok());

        client.shutdown().await;
        Ok(())
    })
    .await
}
