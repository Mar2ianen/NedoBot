use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;

use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, ContentBlock, Tool},
    service::RunningService,
    transport::TokioChildProcess,
};
use serde_json::Value;
use tokio::process::Command;
use tokio::time::timeout;

use crate::config::Config;

const MAX_TOOL_RESULT_CHARS: usize = 12_000;
const MAX_TOOL_CATALOG_CHARS: usize = 12_000;

/// RMCP client for the scoped chat read-model child process.
///
/// `TokioChildProcess` owns child cleanup: closing or dropping the running
/// service closes the transport and kills an unresponsive child process.
pub struct McpClient {
    service: RunningService<RoleClient, ()>,
    tool_names: HashSet<String>,
    tool_catalog: String,
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
        let tool_names = tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<HashSet<_>>();
        anyhow::ensure!(
            !tool_names.is_empty(),
            "chat DB MCP did not advertise any tools"
        );

        Ok(Self {
            service,
            tool_names,
            tool_catalog: format_tool_catalog(&tools)?,
            timeout: timeout_duration,
        })
    }

    pub fn has_tool(&self, tool: &str) -> bool {
        self.tool_names.contains(tool)
    }

    pub fn tool_names(&self) -> impl Iterator<Item = &str> {
        self.tool_names.iter().map(String::as_str)
    }

    pub fn tool_catalog(&self) -> &str {
        &self.tool_catalog
    }

    pub async fn shutdown(mut self) {
        if let Err(err) = self.service.close_with_timeout(self.timeout).await {
            tracing::warn!(%err, "failed to close chat DB MCP client");
        }
    }

    pub async fn call(&self, tool: &str, arguments: Value) -> anyhow::Result<String> {
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
        render_tool_result(result)
    }
}

fn format_tool_catalog(tools: &[Tool]) -> anyhow::Result<String> {
    let mut catalog = String::new();
    for tool in tools {
        let description = tool
            .description
            .as_deref()
            .unwrap_or("описание отсутствует");
        let input_schema = serde_json::to_string(&tool.input_schema)?;
        catalog.push_str(&format!(
            "- {}: {description}\n  input_schema: {input_schema}\n",
            tool.name
        ));
    }
    Ok(limit_chars(&catalog, MAX_TOOL_CATALOG_CHARS))
}

fn render_tool_result(result: rmcp::model::CallToolResult) -> anyhow::Result<String> {
    anyhow::ensure!(
        result.is_error != Some(true),
        "chat DB MCP returned a tool-level error"
    );
    if let Some(structured_content) = result.structured_content {
        return Ok(limit_chars(
            &serde_json::to_string(&structured_content)?,
            MAX_TOOL_RESULT_CHARS,
        ));
    }

    let content = result
        .content
        .iter()
        .filter_map(content_text)
        .collect::<Vec<_>>()
        .join("\n");
    anyhow::ensure!(!content.is_empty(), "chat DB MCP returned no text result");
    Ok(limit_chars(&content, MAX_TOOL_RESULT_CHARS))
}

fn content_text(content: &ContentBlock) -> Option<String> {
    match content {
        ContentBlock::Text(text) => Some(text.text.clone()),
        ContentBlock::Resource(resource) => Some(resource.get_text()),
        _ => None,
    }
}

fn limit_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let result = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{result}\n[результат MCP обрезан до безопасного лимита]")
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_long_tool_results() {
        let value = limit_chars("а".repeat(10).as_str(), 4);
        assert_eq!(value, "аааа\n[результат MCP обрезан до безопасного лимита]");
    }
}
