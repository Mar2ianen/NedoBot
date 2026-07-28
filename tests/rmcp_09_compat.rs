//! RMCP-09 compatibility coverage for the inactive RMCP adapters.
//!
//! The suite deliberately uses a lazy, unreachable pool. Catalog operations are
//! manifest-only and must not acquire a database connection; data-bearing tools
//! remain covered by the ignored PostgreSQL integration tests.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result};
use rmcp::{
    ServiceError, ServiceExt,
    model::{CallToolRequestParams, ErrorCode},
};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tg_ai_bot_teloxide::{
    features::chat_read_api::{ChatReadApi, catalog::PublicCatalog},
    mcp::server::ChatMcpServer,
};

const LEGACY_HTTP_CONTRACT: &str = include_str!("fixtures/mcp/nedonews_mcp_http.json");
const LEGACY_STDIO_CONTRACT: &str = include_str!("fixtures/mcp/chat_db_mcp.json");

// These tool names overlap, but their argument contracts intentionally do not.
// The old adapters accept chat IDs or legacy DB request shapes, whereas the
// scoped read-model and RMCP-generated schemas omit or type them differently.
const EXPECTED_INCOMPATIBLE_INPUT_SCHEMAS: &[&str] = &[
    "legacy HTTP: chat.get_message",
    "legacy HTTP: chat.get_user_profile",
    "legacy HTTP: chat.search_messages",
    "legacy HTTP: db.aggregate",
    "legacy HTTP: db.count",
    "legacy HTTP: db.describe_table",
    "legacy HTTP: db.fetch_row",
    "legacy HTTP: db.search_text",
    "legacy HTTP: db.select",
    "legacy stdio: chat.get_message",
    "legacy stdio: chat.get_message_context",
    "legacy stdio: chat.get_recent_messages",
    "legacy stdio: chat.get_reply_thread",
    "legacy stdio: chat.get_user_interactions",
    "legacy stdio: chat.get_user_profile",
    "legacy stdio: chat.resolve_user",
    "legacy stdio: chat.search_messages",
    "legacy stdio: chat.search_messages_batch",
    "legacy stdio: notes.list_user",
];

fn test_server() -> Result<ChatMcpServer> {
    let manifest = format!("{}/config/mcp_db_manifest.toml", env!("CARGO_MANIFEST_DIR"));
    let catalog = PublicCatalog::load(&manifest)?;
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@localhost/unused")
        .context("RMCP-09 test lazy database URL must be valid")?;
    let api = ChatReadApi::new(pool, catalog.scope(), catalog)?;
    Ok(ChatMcpServer::new(Arc::new(api)))
}

fn fixture_tools(fixture: &str) -> Result<BTreeMap<String, Value>> {
    let fixture: Value = serde_json::from_str(fixture)?;
    fixture["tools/list"]["tools"]
        .as_array()
        .context("fixture tools/list.tools must be an array")?
        .iter()
        .map(|tool| {
            let name = tool["name"]
                .as_str()
                .context("fixture tool must have a name")?
                .to_owned();
            Ok((name, tool.clone()))
        })
        .collect()
}

fn tools_by_name(tools: &[rmcp::model::Tool]) -> Result<BTreeMap<String, Value>> {
    tools
        .iter()
        .map(|tool| {
            let value = serde_json::to_value(tool)?;
            let name = value["name"]
                .as_str()
                .context("RMCP tool must have a name")?
                .to_owned();
            Ok((name, value))
        })
        .collect()
}

fn input_schema(tool: &Value) -> Result<&Value> {
    tool.get("inputSchema")
        .context("tool must publish inputSchema")
}

fn contract_schema(schema: &Value) -> Value {
    let mut schema = schema.clone();
    remove_schema_dialect_marker(&mut schema);
    schema
}

fn remove_schema_dialect_marker(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("$schema");
            if object.get("properties").is_some_and(|properties| {
                properties
                    .as_object()
                    .is_some_and(|properties| properties.is_empty())
            }) {
                object.remove("properties");
            }
            for value in object.values_mut() {
                remove_schema_dialect_marker(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                remove_schema_dialect_marker(value);
            }
        }
        _ => {}
    }
}

fn text_result(result: &rmcp::model::CallToolResult) -> Result<&str> {
    result
        .content
        .first()
        .and_then(|content| content.as_text())
        .map(|text| text.text.as_str())
        .context("tool result must contain text content")
}

fn structured_content_matches_first_text(result: &rmcp::model::CallToolResult) -> Result<Value> {
    let structured_content = result
        .structured_content
        .as_ref()
        .context("tool result must include structured_content")?;
    let text_content: Value = serde_json::from_str(text_result(result)?)
        .context("first tool text content must contain JSON")?;
    assert_eq!(
        structured_content, &text_content,
        "structured_content must semantically match the first text content"
    );
    Ok(text_content)
}

fn arguments(value: Value) -> serde_json::Map<String, Value> {
    value
        .as_object()
        .expect("test tool arguments must be a JSON object")
        .clone()
}

#[tokio::test]
async fn rmcp_09_preserves_legacy_fixture_tool_names_and_input_schemas_where_they_overlap()
-> Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = tokio::spawn(async move {
        test_server()?
            .serve(server_transport)
            .await?
            .waiting()
            .await?;
        anyhow::Ok(())
    });

    // `ServiceExt::serve` performs RMCP's preferred initialize lifecycle before
    // tools/list, unlike the handwritten legacy adapters' fixture snapshots.
    let mut client = ().serve(client_transport).await?;
    let rmcp_tools = tools_by_name(&client.list_tools(None).await?.tools)?;
    let mut incompatible_schemas = BTreeSet::new();

    for (adapter, fixture) in [
        ("legacy HTTP", LEGACY_HTTP_CONTRACT),
        ("legacy stdio", LEGACY_STDIO_CONTRACT),
    ] {
        let legacy_tools = fixture_tools(fixture)?;
        let overlap = legacy_tools
            .keys()
            .filter(|name| rmcp_tools.contains_key(*name))
            .collect::<BTreeSet<_>>();
        assert!(
            !overlap.is_empty(),
            "{adapter} fixture must share at least one tool with ChatMcpServer"
        );

        for name in overlap {
            if contract_schema(input_schema(&legacy_tools[name])?)
                != contract_schema(input_schema(&rmcp_tools[name])?)
            {
                incompatible_schemas.insert(format!("{adapter}: {name}"));
            }
        }
    }

    let legacy_http = fixture_tools(LEGACY_HTTP_CONTRACT)?;
    let legacy_stdio = fixture_tools(LEGACY_STDIO_CONTRACT)?;
    let rmcp_names = rmcp_tools.keys().cloned().collect::<BTreeSet<_>>();
    let http_only = legacy_http
        .keys()
        .filter(|name| !rmcp_names.contains(*name))
        .cloned()
        .collect::<BTreeSet<_>>();
    let stdio_only = legacy_stdio
        .keys()
        .filter(|name| !rmcp_names.contains(*name))
        .cloned()
        .collect::<BTreeSet<_>>();

    // The canary intentionally has no static-file resolver, so this legacy-only
    // URL tool is not a portable ChatMcpServer contract.
    assert_eq!(
        http_only,
        BTreeSet::from(["chat.get_user_avatar".to_owned()])
    );
    assert!(
        stdio_only.is_empty(),
        "unexpected stdio-only tools: {stdio_only:?}"
    );
    let expected_incompatible = EXPECTED_INCOMPATIBLE_INPUT_SCHEMAS
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        incompatible_schemas, expected_incompatible,
        "shared tool schema compatibility changed; review each new or resolved difference"
    );

    client.close().await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn rmcp_09_preferred_lifecycle_calls_safe_catalog_tools_without_database() -> Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = tokio::spawn(async move {
        test_server()?
            .serve(server_transport)
            .await?
            .waiting()
            .await?;
        anyhow::Ok(())
    });

    let mut client = ().serve(client_transport).await?;
    let tools = client.list_tools(None).await?;
    assert!(tools.tools.iter().any(|tool| tool.name == "db.list_tables"));
    assert!(
        tools
            .tools
            .iter()
            .any(|tool| tool.name == "db.describe_table")
    );

    let listed = client
        .call_tool(CallToolRequestParams::new("db.list_tables"))
        .await?;
    assert!(!listed.is_error.unwrap_or(false));
    let tables = structured_content_matches_first_text(&listed)?;
    let table = tables["tables"]
        .as_array()
        .and_then(|tables| tables.first())
        .and_then(|table| table["name"].as_str())
        .context("db.list_tables must return a manifest table")?;

    let described = client
        .call_tool(
            CallToolRequestParams::new("db.describe_table")
                .with_arguments(arguments(serde_json::json!({"table": table}))),
        )
        .await?;
    assert!(!described.is_error.unwrap_or(false));
    let description = structured_content_matches_first_text(&described)?;
    assert_eq!(description["name"], table);

    let rejected = client
        .call_tool(
            CallToolRequestParams::new("db.describe_table")
                .with_arguments(arguments(serde_json::json!({"table": "not_reviewed"}))),
        )
        .await
        .expect_err("unknown catalog table must produce an RMCP invalid-params error");
    let ServiceError::McpError(error) = rejected else {
        anyhow::bail!("unknown catalog table must produce an RMCP protocol error");
    };
    assert_eq!(
        error.code,
        ErrorCode::INVALID_PARAMS,
        "unknown catalog table must return JSON-RPC InvalidParams (-32602)"
    );

    client.close().await?;
    server.await??;
    Ok(())
}
