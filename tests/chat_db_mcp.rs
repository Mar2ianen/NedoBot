use std::{
    collections::BTreeSet, ffi::OsString, future::Future, path::PathBuf, process::Output,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sqlx::{postgres::PgPoolOptions, query};
use tg_ai_bot_teloxide::{config::Config, features::ask::mcp_client::McpClient};

const E2E_TIMEOUT: Duration = Duration::from_secs(10);
const TWO_INSTANCE_TIMEOUT: Duration = Duration::from_secs(120);
static MCP_TEST_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct TestEnvironmentRestore(Vec<(&'static str, Option<OsString>)>);

impl TestEnvironmentRestore {
    fn capture(keys: &[&'static str]) -> Self {
        Self(
            keys.iter()
                .map(|key| (*key, std::env::var_os(key)))
                .collect(),
        )
    }
}

impl Drop for TestEnvironmentRestore {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            unsafe {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }
}

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
    let _env_guard = MCP_TEST_ENV_LOCK.lock().await;
    let _restore_env =
        TestEnvironmentRestore::capture(&["LLM_PROFILES_PATH", "ASK_DATABASE_URL", "MCP_MANIFEST"]);
    within_timeout("McpClient real RMCP child lifecycle", async {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must be set by scripts/test.sh");
        let manifest = manifest_path();

        // McpClient clears the child environment. Database URL and manifest are
        // inherited from the explicit allowlist; instance scope comes from Config.
        unsafe {
            std::env::set_var("ASK_DATABASE_URL", &database_url);
            std::env::set_var("MCP_MANIFEST", &manifest);
        }
        let mut config = Config::from_env()?;
        config.ask_db_mcp_command = Some(env!("CARGO_BIN_EXE_chat_db_mcp").to_string());
        config.ask_db_mcp_args = Vec::new();
        config.ask_db_mcp_env = vec!["ASK_DATABASE_URL".to_string(), "MCP_MANIFEST".to_string()];
        config.ask_db_mcp_timeout_sec = E2E_TIMEOUT.as_secs();
        config.youtube_subtitles_enabled = false;

        let client = McpClient::start(&config).await?;
        assert!(client.has_tool("chat.resolve_user"));
        assert!(!client.has_tool("db.list_tables"));
        assert!(!client.has_tool("youtube.get_subtitles"));

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
        assert!(search["next_offset"].is_null() || search["next_offset"].is_i64());
        assert!(search["scan_limit_reached"].is_boolean());
        let batch = call_object(
            &client,
            "chat.search_messages_batch",
            json!({"queries": ["бот"], "limit_per_query": 1}),
        )
        .await?;
        assert!(batch["results"].is_array());
        assert!(batch["results"][0]["total_count"].is_i64());
        assert!(batch["results"][0]["has_more"].is_boolean());
        assert!(batch["results"][0]["scan_limit_reached"].is_boolean());
        let quality_batch = call_object(
            &client,
            "chat.search_messages_batch",
            json!({
                "queries": [
                    "hybrid quality marker alpha",
                    "nedobot quality anonymous forwarded marker"
                ],
                "include_forwards": true,
                "match_mode": "literal",
                "limit_per_query": 1
            }),
        )
        .await?;
        let quality_results = quality_batch["results"]
            .as_array()
            .context("quality batch must return two results")?;
        assert_eq!(quality_results[0]["total_count"], 3);
        assert_eq!(quality_results[0]["has_more"], true);
        assert_eq!(quality_results[1]["total_count"], 1);
        assert_eq!(quality_results[1]["has_more"], false);
        let count = call_object(&client, "chat.count_messages", json!({"query": "бот"})).await?;
        assert!(count["count"].is_i64());
        assert_eq!(count["unit"], "messages");
        let words = call_object(
            &client,
            "chat.count_word_occurrences",
            json!({"query":"бот"}),
        )
        .await?;
        assert!(words["count"].is_i64());
        assert_eq!(words["unit"], "word_occurrences");
        let user_count =
            call_object(&client, "chat.count_messages", json!({"user_id": user_id})).await?;
        assert!(user_count["count"].is_i64());
        let message = call_object(
            &client,
            "chat.get_message",
            json!({"message_id": message_id}),
        )
        .await?;
        assert_eq!(message["found"], true);
        assert_eq!(message["message"]["message_id"], message_id);
        assert!(message["message"]["author_name"].is_string());
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
        assert_eq!(thread["root_message_id"], message_id);
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
        assert_youtube_child(&mut config).await?;
        Ok(())
    })
    .await
}

#[cfg(unix)]
async fn assert_youtube_child(config: &mut Config) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir()?;
    let text = "Полная расшифровка через настоящий MCP 🦀. ".repeat(1000);
    std::fs::write(
        temp.path().join("source.vtt"),
        format!("WEBVTT\n\n00:00:00.000 --> 01:40:00.000\n{text}\n"),
    )?;
    let command = temp.path().join("fixture-yt-dlp");
    std::fs::write(
        &command,
        r##"#!/bin/sh
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then shift; output="$1"; fi
  shift
done
printf x >> "$root/calls"
cp "$root/source.vtt" "$(dirname -- "$output")/fixture.ru.vtt"
"##,
    )?;
    std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o755))?;
    config.youtube_subtitles_enabled = true;
    config.youtube_subtitles_command = Some(command.to_str().unwrap().to_owned());
    config.youtube_subtitles_languages = vec!["ru".into()];
    config.youtube_subtitles_timeout_sec = 5;
    config.youtube_subtitles_max_chars = 12000;
    config.youtube_subtitles_max_videos = 2;
    let client = McpClient::start(config).await?;
    ensure!(client.has_tool("youtube.get_subtitles"));
    ensure!(client.has_tool("voice.list_transcripts"));
    ensure!(client.has_tool("memory.list_notes"));
    // Generic DB tools остаются вне ASK allowlist даже после расширения MCP.
    ensure!(!client.has_tool("db.read_text"));
    let mut offset = 0;
    let mut version = Value::Null;
    let mut recovered = String::new();
    loop {
        let value = call_object(&client,"youtube.get_subtitles",json!({"urls":["https://youtu.be/abcdefghijk"],"offset":offset,"max_chars":3000,"transcript_id":version})).await?;
        let page = &value["videos"][0];
        recovered.push_str(
            page["transcript"]
                .as_str()
                .context("subtitle page required")?,
        );
        ensure!(page["subtitle_end_seconds"] == 6000.0);
        version = page["transcript_id"].clone();
        if page["has_more"] == false {
            break;
        }
        offset = page["next_offset"]
            .as_u64()
            .context("continuation required")?;
    }
    ensure!(recovered == text.trim());
    ensure!(std::fs::read_to_string(temp.path().join("calls"))? == "x");
    client.shutdown().await;
    Ok(())
}

#[ignore = "requires two clean migrated TEST_DATABASE_URL instances and local MCP binaries"]
#[tokio::test]
async fn independent_community_databases_generate_manifests_and_start_isolated_mcp_servers()
-> Result<()> {
    let _env_guard = MCP_TEST_ENV_LOCK.lock().await;
    let _restore_env =
        TestEnvironmentRestore::capture(&["LLM_PROFILES_PATH", "ASK_DATABASE_URL", "MCP_MANIFEST"]);
    tokio::time::timeout(TWO_INSTANCE_TIMEOUT, async {
        let database_a = std::env::var("TEST_DATABASE_URL")
            .context("TEST_DATABASE_URL must be set by scripts/test.sh")?;
        let database_b = std::env::var("TEST_DATABASE_URL_B")
            .context("TEST_DATABASE_URL_B must be set by scripts/test.sh")?;
        let temp = tempfile::tempdir()?;
        let profile_a =
            write_instance_profile(temp.path(), "community_a", -900_000_101, -900_000_102)?;
        let profile_b =
            write_instance_profile(temp.path(), "community_b", -900_000_201, -900_000_202)?;
        let manifest_a = temp.path().join("community_a_manifest.toml");
        let manifest_b = temp.path().join("community_b_manifest.toml");

        provision_instance(&database_a, &profile_a).await?;
        provision_instance(&database_b, &profile_b).await?;
        generate_manifest(&database_a, &profile_a, &manifest_a).await?;
        generate_manifest(&database_b, &profile_b, &manifest_b).await?;

        let user_a = unique_user_id();
        let user_b = unique_user_id();
        let message_id_a = unique_message_id();
        let message_id_b = unique_message_id();
        let marker_a = format!("community-a-marker-{}", uuid::Uuid::new_v4());
        let marker_b = format!("community-b-marker-{}", uuid::Uuid::new_v4());
        seed_instance_data(
            &database_a,
            -900_000_101,
            message_id_a,
            user_a,
            &marker_a,
            "community-a-chat-note",
            "community-a-user-note",
        )
        .await?;
        seed_instance_data(
            &database_b,
            -900_000_201,
            message_id_b,
            user_b,
            &marker_b,
            "community-b-chat-note",
            "community-b-user-note",
        )
        .await?;

        let config_a = config_for_instance(&profile_a, &database_a, &manifest_a)?;
        ensure!(
            McpClient::start_for_scope(&config_a, -900_000_102)
                .await
                .is_err(),
            "/ask must reject a secondary chat before starting MCP retrieval"
        );
        let client_a = McpClient::start_for_scope(&config_a, -900_000_101).await?;
        let config_b = config_for_instance(&profile_b, &database_b, &manifest_b)?;
        let client_b = McpClient::start_for_scope(&config_b, -900_000_201).await?;
        ensure!(client_a.has_tool("chat.search_messages"));
        ensure!(client_b.has_tool("chat.search_messages"));

        assert_instance_reads(
            &client_a,
            &marker_a,
            user_a,
            "community-a-chat-note",
            "community-a-user-note",
        )
        .await?;
        assert_instance_reads(
            &client_b,
            &marker_b,
            user_b,
            "community-b-chat-note",
            "community-b-user-note",
        )
        .await?;
        assert_instance_hides(
            &client_a,
            &marker_b,
            user_b,
            "community-b-chat-note",
            "community-b-user-note",
        )
        .await?;
        assert_instance_hides(
            &client_b,
            &marker_a,
            user_a,
            "community-a-chat-note",
            "community-a-user-note",
        )
        .await?;
        let cross_search = call_object(
            &client_a,
            "chat.search_messages",
            json!({"query": marker_b, "match_mode": "literal", "limit": 5}),
        )
        .await?;
        ensure!(
            cross_search["total_count"].as_i64() == Some(0),
            "community A must not find community B messages"
        );

        // A running server holds a database shared lease. The provisioner must
        // refuse a scope change instead of retargeting this process in place.
        let changed_live_scope = provision_instance_to_scope(&database_a, &profile_b, true).await?;
        ensure!(
            !changed_live_scope.status.success(),
            "scope reprovisioning must fail while an MCP server is running"
        );
        assert_instance_reads(
            &client_a,
            &marker_a,
            user_a,
            "community-a-chat-note",
            "community-a-user-note",
        )
        .await?;

        client_a.shutdown().await;
        client_b.shutdown().await;

        // A valid scope paired with the other database or manifest must fail
        // during MCP initialization before any tool can be used.
        let wrong_database = config_for_instance(&profile_a, &database_b, &manifest_a)?;
        ensure!(
            McpClient::start_for_scope(&wrong_database, -900_000_101)
                .await
                .is_err(),
            "community A config must not start against community B database"
        );
        let wrong_manifest = config_for_instance(&profile_a, &database_a, &manifest_b)?;
        ensure!(
            McpClient::start_for_scope(&wrong_manifest, -900_000_101)
                .await
                .is_err(),
            "community A config must not start with community B manifest"
        );

        restore_example_scope(&database_a).await?;
        restore_example_scope(&database_b).await?;
        Ok(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("two-instance MCP isolation exceeded {TWO_INSTANCE_TIMEOUT:?}"))?
}

fn write_instance_profile(
    directory: &std::path::Path,
    instance_id: &str,
    chat_id: i64,
    source_channel_id: i64,
) -> Result<PathBuf> {
    let example = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/llm_profiles.toml.example"),
    )?;
    let instance_replaced =
        example.replace("id = \"nedonews\"", &format!("id = \"{instance_id}\""));
    ensure!(
        instance_replaced != example,
        "example profile instance marker must exist"
    );
    let chat_replaced =
        instance_replaced.replace("id = -1001932061163", &format!("id = {chat_id}"));
    ensure!(
        chat_replaced != instance_replaced,
        "example profile chat marker must exist"
    );
    let profile = chat_replaced.replace(
        "source_channel_id = -1001575496091",
        &format!("source_channel_id = {source_channel_id}"),
    );
    ensure!(
        profile != chat_replaced,
        "example profile source marker must exist"
    );
    let profile = profile
        .replace(
            "agent_profile = \"nedonews_default\"",
            "agent_profile = \"agent_default\"",
        )
        .replace(
            "[agent_profiles.nedonews_default]",
            "[agent_profiles.agent_default]",
        )
        .replace("identity = \"NedoBot\"", "identity = \"Test Agent\"")
        .replace(
            "display_name = \"NedoBot\"",
            "display_name = \"Test Agent\"",
        )
        .replace(
            "community_context = \"НедоNews Chat (разрешена только его история)\"",
            "community_context = \"Synthetic community\"",
        )
        .replace(
            "system_prompt_file = \"builtin:nedonews_ask\"",
            "system_prompt_file = \"agent_prompt.md\"",
        )
        .replace(
            "capability_document_file = \"builtin:nedonews_changelog\"\n",
            "",
        )
        .replace(
            "[agent_profiles.nedonews_default.budgets]",
            "[agent_profiles.agent_default.budgets]",
        )
        .replace(
            "[agent_profiles.nedonews_default.memory]",
            "[agent_profiles.agent_default.memory]",
        );
    std::fs::write(directory.join("agent_prompt.md"), "Synthetic test agent.")?;
    let path = directory.join(format!("{instance_id}_profile.toml"));
    std::fs::write(&path, profile)?;
    Ok(path)
}

fn config_for_instance(profile: &PathBuf, database: &str, manifest: &PathBuf) -> Result<Config> {
    unsafe {
        std::env::set_var("LLM_PROFILES_PATH", profile);
        std::env::set_var("ASK_DATABASE_URL", database);
        std::env::set_var("MCP_MANIFEST", manifest);
    }
    let mut config = Config::from_env()?;
    config.ask_db_mcp_command = Some(env!("CARGO_BIN_EXE_chat_db_mcp").to_owned());
    config.ask_db_mcp_args.clear();
    config.ask_db_mcp_env = vec!["ASK_DATABASE_URL".into(), "MCP_MANIFEST".into()];
    config.ask_db_mcp_timeout_sec = E2E_TIMEOUT.as_secs();
    config.youtube_subtitles_enabled = false;
    Ok(config)
}

async fn provision_instance(database: &str, profile: &PathBuf) -> Result<()> {
    let output = provision_instance_to_scope(database, profile, true).await?;
    ensure!(
        output.status.success(),
        "instance provisioning failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

async fn provision_instance_to_scope(
    database: &str,
    profile: &PathBuf,
    confirm: bool,
) -> Result<Output> {
    let mut command =
        tokio::process::Command::new(env!("CARGO_BIN_EXE_provision_mcp_instance_scope"));
    command
        .env("DATABASE_URL", database)
        .env("LLM_PROFILES_PATH", profile);
    if confirm {
        command.arg("--confirm-scope-change");
    }
    Ok(command.output().await?)
}

async fn generate_manifest(
    database: &str,
    profile: &PathBuf,
    output: &std::path::Path,
) -> Result<()> {
    let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_generate_mcp_db_manifest"))
        .arg(output)
        .env("DATABASE_URL", database)
        .env("LLM_PROFILES_PATH", profile)
        .output()
        .await?;
    ensure!(
        result.status.success(),
        "manifest generation failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    ensure!(output.is_file(), "generator must write its output manifest");
    Ok(())
}

async fn seed_instance_data(
    database: &str,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    message: &str,
    chat_note: &str,
    user_note: &str,
) -> Result<()> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(database)
        .await?;
    query("insert into telegram_user_profiles (telegram_user_id, first_name) values ($1, $2)")
        .bind(user_id)
        .bind(message)
        .execute(&pool)
        .await?;
    query("insert into telegram_messages (chat_id, message_id, user_id, text) values ($1, $2, $3, $4)")
        .bind(chat_id)
        .bind(message_id)
        .bind(user_id)
        .bind(message)
        .execute(&pool)
        .await?;
    query("insert into telegram_chat_notes (chat_id, note, created_by_user_id) values ($1, $2, 1)")
        .bind(chat_id)
        .bind(chat_note)
        .execute(&pool)
        .await?;
    query("insert into telegram_user_notes (chat_id, telegram_user_id, note, created_by_user_id) values ($1, $2, $3, 1)")
        .bind(chat_id)
        .bind(user_id)
        .bind(user_note)
        .execute(&pool)
        .await?;
    Ok(())
}

fn unique_user_id() -> i64 {
    let bytes = uuid::Uuid::new_v4();
    let mut id_bytes = [0_u8; 8];
    id_bytes[1..].copy_from_slice(&bytes.as_bytes()[..7]);
    i64::from_be_bytes(id_bytes).max(1)
}

fn unique_message_id() -> i32 {
    let bytes = uuid::Uuid::new_v4();
    i32::from_be_bytes([
        0,
        bytes.as_bytes()[0],
        bytes.as_bytes()[1],
        bytes.as_bytes()[2],
    ])
    .max(1)
}

async fn assert_instance_reads(
    client: &McpClient,
    marker: &str,
    user_id: i64,
    chat_note: &str,
    user_note: &str,
) -> Result<()> {
    let search = call_object(
        client,
        "chat.search_messages",
        json!({"query": marker, "match_mode": "literal", "limit": 5}),
    )
    .await?;
    ensure!(search["total_count"].as_i64() == Some(1));
    ensure!(search["messages"][0]["text"] == marker);
    let profile = call_object(
        client,
        "chat.get_user_profile",
        json!({"telegram_user_id": user_id}),
    )
    .await?;
    ensure!(profile["found"] == true);
    let chat_notes = call_object(client, "notes.list_chat", json!({})).await?;
    ensure!(
        chat_notes["notes"]
            .as_array()
            .is_some_and(|notes| notes.iter().any(|note| note["note"] == chat_note))
    );
    let user_notes = call_object(
        client,
        "notes.list_user",
        json!({"telegram_user_id": user_id}),
    )
    .await?;
    ensure!(
        user_notes["notes"]
            .as_array()
            .is_some_and(|notes| notes.iter().any(|note| note["note"] == user_note))
    );
    Ok(())
}

async fn assert_instance_hides(
    client: &McpClient,
    marker: &str,
    user_id: i64,
    chat_note: &str,
    user_note: &str,
) -> Result<()> {
    let search = call_object(
        client,
        "chat.search_messages",
        json!({"query": marker, "match_mode": "literal", "limit": 5}),
    )
    .await?;
    ensure!(search["total_count"].as_i64() == Some(0));
    let profile = call_object(
        client,
        "chat.get_user_profile",
        json!({"telegram_user_id": user_id}),
    )
    .await?;
    ensure!(profile["found"] == false);
    let chat_notes = call_object(client, "notes.list_chat", json!({})).await?;
    ensure!(
        chat_notes["notes"]
            .as_array()
            .is_some_and(|notes| notes.iter().all(|note| note["note"] != chat_note))
    );
    let user_notes = call_object(
        client,
        "notes.list_user",
        json!({"telegram_user_id": user_id}),
    )
    .await?;
    ensure!(
        user_notes["notes"]
            .as_array()
            .is_some_and(|notes| notes.iter().all(|note| note["note"] != user_note))
    );
    Ok(())
}

async fn restore_example_scope(database: &str) -> Result<()> {
    let example =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/llm_profiles.toml.example");
    let output = provision_instance_to_scope(database, &example, true).await?;
    ensure!(
        output.status.success(),
        "cannot restore test DB scope: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
