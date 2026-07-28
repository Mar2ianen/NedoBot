use std::{future::Future, path::PathBuf, time::Duration};

use anyhow::Result;
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};

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
async fn rmcp_child_binary_initializes_lists_calls_and_closes() -> Result<()> {
    within_timeout("chat_db_mcp_rmcp child lifecycle", async {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must be set by scripts/test.sh");
        let working_directory = tempfile::tempdir()?;
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_chat_db_mcp_rmcp"));
        command
            .env("ASK_DATABASE_URL", database_url)
            .env("MCP_MANIFEST", manifest_path())
            .current_dir(&working_directory);

        let transport = TokioChildProcess::new(command)?;
        let mut client = ().serve(transport).await?;

        let tools = client.list_tools(None).await?;
        assert!(tools.tools.iter().any(|tool| tool.name == "db.list_tables"));

        let result = client
            .call_tool(CallToolRequestParams::new("db.list_tables"))
            .await?;
        assert!(!result.is_error.unwrap_or(false));
        assert!(
            result
                .content
                .first()
                .and_then(|content| content.as_text())
                .is_some(),
            "catalog tool must return a text result"
        );

        client.close().await?;
        Ok(())
    })
    .await
}
