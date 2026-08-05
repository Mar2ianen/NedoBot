use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::time::Duration;

use genai::chat::Tool as GenAiTool;
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, ContentBlock, Tool},
    service::RunningService,
    transport::TokioChildProcess,
};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::timeout;

use crate::config::Config;
use crate::features::chat_read_api::types::{
    CHAT_EMBEDDING_MODEL_ENV, CHAT_EMBEDDING_TIMEOUT_ENV, CHAT_EMBEDDING_URL_ENV,
};

// Reserve space for the agent's untrusted-result envelope; never raw-truncate JSON there.
const MAX_TOOL_RESULT_CHARS: usize = 11_000;
const MAX_TOOL_CATALOG_CHARS: usize = 12_000;
const PROVIDER_TOOL_NAME_SEPARATOR: &str = "__";

pub const LOCAL_AGENT_TOOLS: &[&str] = &["notes.add_user", "web.search", "github.search"];
pub const ASK_MCP_TOOL_ALLOWLIST: &[&str] = &[
    "chat.resolve_user",
    "chat.get_user_profile",
    "chat.count_messages",
    "chat.search_messages",
    "chat.search_messages_batch",
    "chat.get_recent_messages",
    "chat.get_message",
    "chat.get_message_context",
    "chat.get_reply_thread",
    "chat.get_user_interactions",
    "notes.list_chat",
    "notes.list_user",
];

/// Full MCP value for policy, audit, and evidence processing plus a bounded,
/// syntactically valid JSON preview that is safe to give to the agent.
pub struct McpToolResult {
    pub value: Value,
    pub agent_preview: String,
}

/// RMCP client for the scoped chat read-model child process.
///
/// `TokioChildProcess` owns child cleanup: closing or dropping the running
/// service closes the transport and kills an unresponsive child process.
pub struct McpClient {
    service: RunningService<RoleClient, ()>,
    tool_names: HashSet<String>,
    genai_tools: Vec<GenAiTool>,
    wire_to_canonical: HashMap<String, String>,
    timeout: Duration,
}

impl McpClient {
    pub async fn start(config: &Config) -> anyhow::Result<Self> {
        let command = config
            .ask_db_mcp_command
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("ASK_DB_MCP_COMMAND is not configured"))?;
        let timeout_duration = Duration::from_secs(config.ask_db_mcp_timeout_sec);
        let mut command = Command::new(command);
        command
            .args(&config.ask_db_mcp_args)
            .env_clear()
            .kill_on_drop(true);
        for name in &config.ask_db_mcp_env {
            if let Ok(value) = std::env::var(name) {
                command.env(name, value);
            }
        }
        command.env("DISCUSSION_CHAT_ID", config.discussion_chat_id.to_string());
        if config.chat_retrieval_embeddings_enabled {
            command
                .env(CHAT_EMBEDDING_URL_ENV, &config.rag_embedding_url)
                .env(CHAT_EMBEDDING_MODEL_ENV, &config.rag_embedding_model)
                .env(
                    CHAT_EMBEDDING_TIMEOUT_ENV,
                    config.rag_embedding_timeout_sec.to_string(),
                );
        }

        let transport = TokioChildProcess::builder(command)
            .stderr(Stdio::null())
            .spawn()?
            .0;
        let service = timeout(timeout_duration, ().serve(transport))
            .await
            .map_err(|_| anyhow::anyhow!("chat DB MCP initialization timed out"))??;
        let tools = timeout(timeout_duration, service.list_all_tools())
            .await
            .map_err(|_| anyhow::anyhow!("chat DB MCP tools/list timed out"))??;
        reject_local_tool_collisions(&tools)?;
        let catalog = format_tool_catalog(&tools)?;
        ensure_required_ask_tools(&catalog)?;
        anyhow::ensure!(
            !catalog.tool_names.is_empty(),
            "chat DB MCP did not advertise an ASK-policy tool"
        );

        Ok(Self {
            service,
            tool_names: catalog.tool_names,
            genai_tools: catalog.genai_tools,
            wire_to_canonical: catalog.wire_to_canonical,
            timeout: timeout_duration,
        })
    }

    pub fn has_tool(&self, tool: &str) -> bool {
        self.tool_names.contains(tool)
    }

    pub fn tool_names(&self) -> impl Iterator<Item = &str> {
        self.tool_names.iter().map(String::as_str)
    }

    pub fn genai_tools(&self) -> &[GenAiTool] {
        &self.genai_tools
    }

    pub fn canonical_tool_name(&self, wire_name: &str) -> Option<&str> {
        self.wire_to_canonical.get(wire_name).map(String::as_str)
    }

    pub async fn shutdown(mut self) {
        if let Err(err) = self.service.close_with_timeout(self.timeout).await {
            tracing::warn!(%err, "failed to close chat DB MCP client");
        }
    }

    pub async fn call(&self, tool: &str, arguments: Value) -> anyhow::Result<McpToolResult> {
        anyhow::ensure!(
            self.has_tool(tool),
            "ask agent requested an unavailable MCP tool"
        );
        let arguments = arguments
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("MCP tool arguments must be an object"))?;
        let request = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
        let result = timeout(self.timeout, self.service.call_tool(request))
            .await
            .map_err(|_| anyhow::anyhow!("chat DB MCP tool call timed out"))??;
        parse_tool_result(result)
    }
}

struct ToolCatalog {
    tool_names: HashSet<String>,
    genai_tools: Vec<GenAiTool>,
    wire_to_canonical: HashMap<String, String>,
}

fn ensure_required_ask_tools(catalog: &ToolCatalog) -> anyhow::Result<()> {
    for required in ["chat.search_messages", "chat.count_messages"] {
        anyhow::ensure!(
            catalog.tool_names.contains(required),
            "chat DB MCP is missing required ASK tool {required}"
        );
    }
    Ok(())
}

/// OpenAI-compatible function names do not accept the dotted MCP namespace.
/// Keep canonical names for policy/audit and use a reversible provider-safe wire name.
pub(crate) fn wire_tool_name(canonical_name: &str) -> String {
    canonical_name.replace('.', PROVIDER_TOOL_NAME_SEPARATOR)
}

fn reject_local_tool_collisions(tools: &[Tool]) -> anyhow::Result<()> {
    if let Some(tool) = tools
        .iter()
        .find(|tool| LOCAL_AGENT_TOOLS.contains(&tool.name.as_ref()))
    {
        anyhow::bail!(
            "chat DB MCP tool {:?} collides with a local ASK tool",
            tool.name
        );
    }
    Ok(())
}

fn format_tool_catalog(tools: &[Tool]) -> anyhow::Result<ToolCatalog> {
    let mut allowed_tools = tools
        .iter()
        .filter(|tool| ASK_MCP_TOOL_ALLOWLIST.contains(&tool.name.as_ref()))
        .collect::<Vec<_>>();
    allowed_tools.sort_unstable_by(|left, right| left.name.cmp(&right.name));

    let mut tool_names = HashSet::new();
    let mut genai_tools = Vec::new();
    let mut wire_to_canonical = HashMap::new();
    let mut rendered_chars = 0;
    for tool in allowed_tools {
        let entry = format_tool_catalog_entry(tool)?;
        if rendered_chars + entry.chars().count() > MAX_TOOL_CATALOG_CHARS {
            continue;
        }
        let canonical_name = tool.name.to_string();
        let wire_name = wire_tool_name(&canonical_name);
        if let Some(previous) = wire_to_canonical.insert(wire_name.clone(), canonical_name.clone())
        {
            anyhow::bail!(
                "MCP tools {:?} and {:?} collide after provider tool-name mapping",
                previous,
                canonical_name
            );
        }
        tool_names.insert(canonical_name);
        rendered_chars += entry.chars().count();
        let genai_tool = GenAiTool::new(wire_name)
            .with_description(
                tool.description
                    .as_deref()
                    .unwrap_or("описание отсутствует"),
            )
            .with_schema(Value::Object((*tool.input_schema).clone()));
        // MCP-схемы содержат optional-поля. Strict OpenAI-compatible tool schema
        // требует объявлять каждое поле обязательным, поэтому policy-проверка
        // остаётся локальной, а provider strict mode для MCP не включаем.
        genai_tools.push(genai_tool);
    }
    Ok(ToolCatalog {
        tool_names,
        genai_tools,
        wire_to_canonical,
    })
}

fn format_tool_catalog_entry(tool: &Tool) -> anyhow::Result<String> {
    let description = tool
        .description
        .as_deref()
        .unwrap_or("описание отсутствует");
    let input_schema = serde_json::to_string(&tool.input_schema)?;
    Ok(format!(
        "- {}: {description}\n  input_schema: {input_schema}\n",
        tool.name
    ))
}

fn parse_tool_result(result: rmcp::model::CallToolResult) -> anyhow::Result<McpToolResult> {
    anyhow::ensure!(
        result.is_error != Some(true),
        "chat DB MCP returned a tool-level error"
    );
    let value = if let Some(structured_content) = result.structured_content {
        structured_content
    } else {
        let content = result
            .content
            .iter()
            .filter_map(content_text)
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::ensure!(!content.is_empty(), "chat DB MCP returned no text result");
        serde_json::from_str(&content).unwrap_or(Value::String(content))
    };
    Ok(McpToolResult {
        agent_preview: structured_preview(&value, MAX_TOOL_RESULT_CHARS)?,
        value,
    })
}

pub(crate) fn structured_preview(value: &Value, limit: usize) -> anyhow::Result<String> {
    let rendered = serde_json::to_string(value)?;
    if rendered.chars().count() <= limit {
        return Ok(rendered);
    }

    for (string_limit, collection_limit, depth_limit) in
        [(2_000, 32, 8), (800, 12, 5), (240, 4, 3), (80, 2, 2)]
    {
        let preview = truncate_value(value, string_limit, collection_limit, depth_limit, 0);
        let rendered = serde_json::to_string(&preview)?;
        if rendered.chars().count() <= limit {
            return Ok(rendered);
        }
    }

    Ok(
        json!({"_truncated": true, "summary": "результат MCP превышает лимит предпросмотра"})
            .to_string(),
    )
}

fn truncate_value(
    value: &Value,
    string_limit: usize,
    collection_limit: usize,
    depth_limit: usize,
    depth: usize,
) -> Value {
    if depth >= depth_limit {
        return json!({"_truncated": true});
    }
    match value {
        Value::String(value) => Value::String(truncate_string(value, string_limit)),
        Value::Array(items) => {
            let mut preview = items
                .iter()
                .take(collection_limit)
                .map(|item| {
                    truncate_value(item, string_limit, collection_limit, depth_limit, depth + 1)
                })
                .collect::<Vec<_>>();
            if items.len() > collection_limit {
                preview.push(json!({"_truncated_items": items.len() - collection_limit}));
            }
            Value::Array(preview)
        }
        Value::Object(items) => {
            let mut preview = serde_json::Map::new();
            for (key, item) in items.iter().take(collection_limit) {
                preview.insert(
                    truncate_string(key, string_limit),
                    truncate_value(item, string_limit, collection_limit, depth_limit, depth + 1),
                );
            }
            if items.len() > collection_limit {
                preview.insert(
                    "_truncated_fields".to_string(),
                    json!(items.len() - collection_limit),
                );
            }
            Value::Object(preview)
        }
        _ => value.clone(),
    }
}

fn truncate_string(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let result = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{result}…")
    } else {
        result
    }
}

fn content_text(content: &ContentBlock) -> Option<String> {
    match content {
        ContentBlock::Text(text) => Some(text.text.clone()),
        ContentBlock::Resource(resource) => Some(resource.get_text()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_truncates_json_structurally() {
        let preview =
            structured_preview(&json!({"messages": ["а".repeat(20_000)]}), 1_000).unwrap();
        let parsed: Value = serde_json::from_str(&preview).unwrap();
        assert!(parsed["messages"][0].as_str().unwrap().ends_with('…'));
        assert!(preview.chars().count() <= 1_000);
    }

    #[test]
    fn catalog_only_includes_policy_tools_as_whole_entries() {
        let tools = vec![
            Tool::new("chat.resolve_user", "schema", serde_json::Map::new()),
            Tool::new("db.select", "schema", serde_json::Map::new()),
        ];
        let catalog = format_tool_catalog(&tools).unwrap();
        assert!(catalog.tool_names.contains("chat.resolve_user"));
        assert!(!catalog.tool_names.contains("db.select"));
        assert_eq!(catalog.genai_tools.len(), 1);
        assert!(catalog.genai_tools[0].schema.is_some());
        assert_eq!(
            catalog.genai_tools[0].name.to_string(),
            "chat__resolve_user"
        );
        assert_eq!(catalog.genai_tools[0].strict, None);
        assert_eq!(
            catalog.wire_to_canonical.get("chat__resolve_user"),
            Some(&"chat.resolve_user".to_string())
        );
    }

    #[test]
    fn local_tool_collision_is_rejected() {
        let error = reject_local_tool_collisions(&[Tool::new(
            "web.search",
            "schema",
            serde_json::Map::new(),
        )])
        .unwrap_err()
        .to_string();
        assert!(error.contains("collides"));
    }

    #[test]
    fn ask_catalog_requires_search_and_count_tools() {
        let catalog = format_tool_catalog(&[Tool::new(
            "chat.search_messages",
            "schema",
            serde_json::Map::new(),
        )])
        .unwrap();
        let error = ensure_required_ask_tools(&catalog).unwrap_err().to_string();
        assert!(error.contains("chat.count_messages"));
    }
}
