use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

use crate::config::Config;
use crate::features::search::provider::SearchProvider;
use crate::features::search::types::{
    MAX_RESULT_SNIPPET_CHARS, MAX_RESULT_TITLE_CHARS, SearchQuery, SearchResult, SearchSource,
};

pub struct McpSearchProvider {
    config: Config,
}

impl McpSearchProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl SearchProvider for McpSearchProvider {
    async fn search(&self, query: &SearchQuery) -> anyhow::Result<Vec<SearchResult>> {
        let timeout_duration = Duration::from_secs(self.config.search_mcp_timeout_sec);
        match timeout(timeout_duration, search_with_mcp(&self.config, query)).await {
            Ok(Ok(results)) => Ok(results),
            Ok(Err(err)) => {
                tracing::warn!(%err, source = ?query.source, "MCP search failed");
                Ok(Vec::new())
            }
            Err(_) => {
                tracing::warn!(source = ?query.source, "MCP search timed out");
                Ok(Vec::new())
            }
        }
    }
}

async fn search_with_mcp(
    config: &Config,
    query: &SearchQuery,
) -> anyhow::Result<Vec<SearchResult>> {
    let command = config
        .search_mcp_command
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("SEARCH_MCP_COMMAND is not configured"))?;
    let mut child = spawn_mcp_process(config, command)?;
    let result = run_mcp_flow(config, query, &mut child).await;

    if let Err(err) = child.kill().await {
        tracing::debug!(%err, "failed to kill MCP child process after search");
    }

    result
}

fn spawn_mcp_process(config: &Config, command: &str) -> anyhow::Result<Child> {
    let mut process = Command::new(command);
    process
        .args(&config.search_mcp_args)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    for name in &config.search_mcp_env {
        if let Ok(value) = std::env::var(name) {
            process.env(name, value);
        }
    }

    Ok(process.spawn()?)
}

async fn run_mcp_flow(
    config: &Config,
    query: &SearchQuery,
    child: &mut Child,
) -> anyhow::Result<Vec<SearchResult>> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("MCP child stdin is unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("MCP child stdout is unavailable"))?;
    let mut stdout = BufReader::new(stdout).lines();

    write_json_line(&mut stdin, &initialize_request()).await?;
    read_json_line(&mut stdout).await?;

    write_json_line(&mut stdin, &initialized_notification()).await?;
    write_json_line(&mut stdin, &tools_call_request(config, query)).await?;
    let response = read_json_line(&mut stdout).await?;

    parse_mcp_tool_response(query.source, &response)
}

fn initialize_request() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "tg-ai-bot-teloxide",
                "version": "0.1.0"
            }
        }
    })
}

fn initialized_notification() -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })
}

fn tools_call_request(config: &Config, query: &SearchQuery) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": tool_name(config, query.source),
            "arguments": {
                "query": query.text,
                "limit": 5
            }
        }
    })
}

fn tool_name(config: &Config, source: SearchSource) -> &str {
    match source {
        SearchSource::Web => &config.search_mcp_tools.web,
        SearchSource::Github => &config.search_mcp_tools.github,
        SearchSource::Reddit => &config.search_mcp_tools.reddit,
    }
}

async fn write_json_line(
    stdin: &mut tokio::process::ChildStdin,
    value: &Value,
) -> anyhow::Result<()> {
    stdin
        .write_all(serde_json::to_string(value)?.as_bytes())
        .await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;
    Ok(())
}

async fn read_json_line(
    stdout: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
) -> anyhow::Result<Value> {
    let line = stdout
        .next_line()
        .await?
        .ok_or_else(|| anyhow::anyhow!("MCP child stdout closed"))?;
    Ok(serde_json::from_str(&line)?)
}

fn parse_mcp_tool_response(
    source: SearchSource,
    response: &Value,
) -> anyhow::Result<Vec<SearchResult>> {
    let Some(content) = response
        .pointer("/result/content")
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };

    let mut results = Vec::new();
    for item in content {
        if item
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "text")
        {
            continue;
        }

        let Some(text) = item.get("text").and_then(Value::as_str) else {
            continue;
        };

        results.extend(parse_tool_output(source, text)?);
    }

    Ok(results)
}

fn parse_tool_output(source: SearchSource, text: &str) -> anyhow::Result<Vec<SearchResult>> {
    let value: Value = serde_json::from_str(text.trim())?;
    let items = match value.as_array() {
        Some(items) => items,
        None => value
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                anyhow::anyhow!("MCP tool output must be array or object with results")
            })?,
    };

    Ok(items
        .iter()
        .filter_map(|item| parse_result(source, item))
        .collect())
}

fn parse_result(source: SearchSource, item: &Value) -> Option<SearchResult> {
    let title = truncate_chars(
        item.get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim(),
        MAX_RESULT_TITLE_CHARS,
    );
    let snippet = truncate_chars(
        item.get("snippet")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim(),
        MAX_RESULT_SNIPPET_CHARS,
    );

    if title.is_empty() && snippet.is_empty() {
        return None;
    }

    Some(SearchResult {
        source,
        title,
        url: item
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
        snippet,
    })
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_array_results() {
        let output = r#"[
            { "title": "Release", "url": "https://example.com/release", "snippet": "Version shipped" }
        ]"#;

        let results = parse_tool_output(SearchSource::Web, output).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, SearchSource::Web);
        assert_eq!(results[0].title, "Release");
        assert_eq!(results[0].url, "https://example.com/release");
        assert_eq!(results[0].snippet, "Version shipped");
    }

    #[test]
    fn parses_object_results() {
        let output = r#"{
            "results": [
                { "title": "Issue", "url": "https://github.com/org/repo/issues/1", "snippet": "Bug discussion" }
            ]
        }"#;

        let results = parse_tool_output(SearchSource::Github, output).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, SearchSource::Github);
        assert_eq!(results[0].title, "Issue");
    }

    #[test]
    fn drops_empty_results() {
        let output = r#"[
            { "title": "", "url": "https://example.com/empty", "snippet": "" },
            { "url": "https://example.com/missing" },
            { "title": "Kept", "url": "https://example.com/kept" }
        ]"#;

        let results = parse_tool_output(SearchSource::Reddit, output).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Kept");
    }

    #[test]
    fn truncates_result_fields() {
        let long_title = "t".repeat(MAX_RESULT_TITLE_CHARS + 10);
        let long_snippet = "s".repeat(MAX_RESULT_SNIPPET_CHARS + 10);
        let output = format!(
            r#"[{{"title":"{long_title}","url":"https://example.com","snippet":"{long_snippet}"}}]"#
        );

        let results = parse_tool_output(SearchSource::Web, &output).unwrap();

        assert_eq!(results[0].title.chars().count(), MAX_RESULT_TITLE_CHARS);
        assert_eq!(results[0].snippet.chars().count(), MAX_RESULT_SNIPPET_CHARS);
    }
}
