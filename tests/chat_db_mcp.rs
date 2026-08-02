use std::{collections::BTreeSet, future::Future, path::PathBuf, time::Duration};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
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

async fn call_object(client: &McpClient, tool: &str, arguments: Value) -> Result<Value> {
    let result = client.call(tool, arguments).await?;
    ensure!(
        result.value.is_object(),
        "{tool} must return an object root"
    );
    serde_json::from_str::<Value>(&result.agent_preview)
        .with_context(|| format!("{tool} preview must remain valid JSON"))?;
    Ok(result.value)
}

#[ignore = "requires TEST_DATABASE_URL and a migrated local PostgreSQL database"]
#[tokio::test]
async fn ask_mcp_client_starts_canonical_rmcp_child_with_env_clear_allowlist() -> Result<()> {
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
        config.ask_db_mcp_command = Some(env!("CARGO_BIN_EXE_chat_db_mcp").to_string());
        config.ask_db_mcp_args = Vec::new();
        config.ask_db_mcp_env = vec!["ASK_DATABASE_URL".to_string(), "MCP_MANIFEST".to_string()];
        config.ask_db_mcp_timeout_sec = E2E_TIMEOUT.as_secs();

        let client = McpClient::start(&config).await?;
        assert!(client.has_tool("chat.resolve_user"));
        assert!(!client.has_tool("db.list_tables"));

        let recent = call_object(&client, "chat.get_recent_messages", json!({"limit": 20})).await?;
        let messages = recent["messages"]
            .as_array()
            .context("recent messages must expose messages")?;
        let message = messages
            .iter()
            .find(|message| message["user_id"].as_i64().is_some())
            .context("test database must contain a public message with a user")?;
        let message_id = message["message_id"]
            .as_i64()
            .context("message_id must be an integer")?;
        let user_id = message["user_id"]
            .as_i64()
            .context("user_id must be an integer")?;
        let user_ids = messages
            .iter()
            .filter_map(|message| message["user_id"].as_i64())
            .collect::<BTreeSet<_>>();
        let second_user_id = user_ids
            .iter()
            .copied()
            .find(|candidate| *candidate != user_id)
            .context("test database must contain messages from two public users")?;

        let search = call_object(
            &client,
            "chat.search_messages",
            json!({"query": "бот", "limit": 1}),
        )
        .await?;
        assert!(search["messages"].is_array());
        assert!(search["total_count"].is_i64());
        assert!(search["has_more"].is_boolean());
        let batch = call_object(
            &client,
            "chat.search_messages_batch",
            json!({"queries": ["бот"], "limit_per_query": 1}),
        )
        .await?;
        assert!(batch["results"].is_array());
        assert!(batch["results"][0]["total_count"].is_i64());
        assert!(batch["results"][0]["has_more"].is_boolean());
        let count = call_object(&client, "chat.count_messages", json!({"query": "бот"})).await?;
        assert!(count["count"].is_i64());
        let message = call_object(
            &client,
            "chat.get_message",
            json!({"message_id": message_id}),
        )
        .await?;
        assert_eq!(message["found"], true);
        assert_eq!(message["message"]["message_id"], message_id);
        let context = call_object(
            &client,
            "chat.get_message_context",
            json!({"message_id": message_id, "before": 1, "after": 1}),
        )
        .await?;
        assert!(context["context"].is_array());
        let thread = call_object(
            &client,
            "chat.get_reply_thread",
            json!({"message_id": message_id}),
        )
        .await?;
        assert!(thread["thread"].is_array());
        let interactions = call_object(
            &client,
            "chat.get_user_interactions",
            json!({"first_user_id": user_id, "second_user_id": second_user_id, "limit": 1}),
        )
        .await?;
        assert!(interactions["interactions"].is_array());
        let profile = call_object(
            &client,
            "chat.get_user_profile",
            json!({"telegram_user_id": user_id}),
        )
        .await?;
        assert_eq!(profile["found"], true);
        let resolved = call_object(
            &client,
            "chat.resolve_user",
            json!({"telegram_user_id": user_id}),
        )
        .await?;
        assert!(resolved["users"].is_array());
        let chat_notes = call_object(&client, "notes.list_chat", json!({})).await?;
        assert!(chat_notes["notes"].is_array());
        let user_notes = call_object(
            &client,
            "notes.list_user",
            json!({"telegram_user_id": user_id}),
        )
        .await?;
        assert!(user_notes["notes"].is_array());

        client.shutdown().await;
        Ok(())
    })
    .await
}
