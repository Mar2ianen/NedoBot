use std::process::Stdio;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
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
    let process_config = McpProcessConfig::for_query(config, query)?;
    let mut child = spawn_mcp_process(&process_config)?;
    let result = run_mcp_flow(config, query, &process_config, &mut child).await;

    if let Err(err) = child.kill().await {
        tracing::debug!(%err, "failed to kill MCP child process after search");
    }

    result
}

struct McpProcessConfig {
    command: String,
    args: Vec<String>,
    env: Vec<String>,
    tool_names: Vec<String>,
    fetch_tool: Option<String>,
}

impl McpProcessConfig {
    fn for_query(config: &Config, query: &SearchQuery) -> anyhow::Result<Self> {
        if query.source == SearchSource::Github {
            if let Some(command) = config.search_github_mcp_command.as_deref() {
                return Ok(Self {
                    command: command.to_string(),
                    args: config.search_github_mcp_args.clone(),
                    env: config.search_github_mcp_env.clone(),
                    tool_names: github_tool_names(config),
                    fetch_tool: None,
                });
            }
        }

        let command = config
            .search_mcp_command
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("SEARCH_MCP_COMMAND is not configured"))?;

        Ok(Self {
            command: command.to_string(),
            args: config.search_mcp_args.clone(),
            env: config.search_mcp_env.clone(),
            tool_names: vec![tool_name(config, query.source).to_string()],
            fetch_tool: config.search_mcp_fetch_tool.clone(),
        })
    }
}

fn github_tool_names(config: &Config) -> Vec<String> {
    config
        .search_github_mcp_tools
        .iter()
        .filter(|tool| is_github_readonly_search_tool(tool))
        .cloned()
        .collect()
}

fn is_github_readonly_search_tool(tool_name: &str) -> bool {
    matches!(tool_name, "search_issues" | "search_code")
}

fn spawn_mcp_process(config: &McpProcessConfig) -> anyhow::Result<Child> {
    let mut process = Command::new(&config.command);
    process
        .args(&config.args)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    for name in &config.env {
        if let Ok(value) = std::env::var(name) {
            process.env(name, value);
        }
    }

    Ok(process.spawn()?)
}

async fn run_mcp_flow(
    config: &Config,
    query: &SearchQuery,
    process_config: &McpProcessConfig,
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

    let mut results = Vec::new();
    let mut request_id = 2;
    for tool_name in &process_config.tool_names {
        write_json_line(
            &mut stdin,
            &tools_call_request(request_id, tool_name, tool_arguments(tool_name, query)),
        )
        .await?;
        request_id += 1;

        let response = read_json_line(&mut stdout).await?;
        results.extend(parse_mcp_tool_response(query.source, &response)?);
    }

    if let Some(fetch_tool) = process_config.fetch_tool.as_deref() {
        let urls = fetch_urls(&results, config.search_fetch_top_n);
        for url in &urls {
            write_json_line(
                &mut stdin,
                &fetch_call_request(
                    request_id,
                    fetch_tool,
                    std::slice::from_ref(url),
                    config.search_fetch_max_chars,
                ),
            )
            .await?;
            request_id += 1;

            let response = read_json_line(&mut stdout).await?;
            let mut fetched_results = parse_mcp_tool_response(query.source, &response)?;
            attach_requested_url(&mut fetched_results, url);
            enrich_results_with_fetch(&mut results, fetched_results);
        }
    }

    if query.source == SearchSource::Github && process_config.fetch_tool.is_none() {
        enrich_github_results(
            &mut stdin,
            &mut stdout,
            &mut request_id,
            &mut results,
            config.search_fetch_top_n,
        )
        .await;
    }

    Ok(results)
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

fn fetch_call_request(id: u64, tool_name: &str, urls: &[String], max_chars: usize) -> Value {
    tools_call_request(
        id,
        tool_name,
        fetch_tool_arguments(tool_name, urls, max_chars),
    )
}

fn tools_call_request(id: u64, tool_name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments
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

fn tool_arguments(tool_name: &str, query: &SearchQuery) -> Value {
    match tool_name {
        "web_search_exa" | "web_search_advanced_exa" => json!({
            "query": query.text,
            "numResults": 5
        }),
        "brave_web_search" | "brave_local_search" => json!({
            "query": query.text,
            "count": 5
        }),
        "search_repositories" => json!({
            "query": query.text,
            "perPage": 5
        }),
        "search_code" | "search_issues" | "search_users" => json!({
            "q": query.text,
            "per_page": 5
        }),
        _ => json!({
            "query": query.text,
            "limit": 5
        }),
    }
}

fn github_fetch_call_request(id: u64, resource: &GithubResource) -> Value {
    match resource {
        GithubResource::Issue {
            owner,
            repo,
            issue_number,
        } => tools_call_request(
            id,
            "get_issue",
            json!({
                "owner": owner,
                "repo": repo,
                "issue_number": issue_number
            }),
        ),
        GithubResource::File {
            owner,
            repo,
            path,
            reference,
        } => tools_call_request(
            id,
            "get_file_contents",
            json!({
                "owner": owner,
                "repo": repo,
                "path": path,
                "ref": reference
            }),
        ),
    }
}

fn fetch_tool_arguments(tool_name: &str, urls: &[String], max_chars: usize) -> Value {
    match tool_name {
        "web_fetch_exa" => json!({
            "urls": urls,
            "maxCharacters": max_chars
        }),
        _ => json!({
            "urls": urls,
            "max_chars": max_chars
        }),
    }
}

fn fetch_urls(results: &[SearchResult], top_n: usize) -> Vec<String> {
    let mut urls = Vec::new();

    for result in results {
        let url = result.url.trim();
        if !is_safe_fetch_url(url) || urls.iter().any(|seen| seen == url) {
            continue;
        }

        urls.push(url.to_string());
        if urls.len() >= top_n {
            break;
        }
    }

    urls
}

pub(crate) fn is_safe_fetch_url(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") || url.username() != "" || url.password().is_some()
    {
        return false;
    }

    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host == "metadata.google.internal"
    {
        return false;
    }

    let Ok(ip) = host.parse::<std::net::IpAddr>() else {
        return true;
    };
    match ip {
        std::net::IpAddr::V4(ip) => {
            let octets = ip.octets();
            !(ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || octets == [169, 254, 169, 254])
        }
        std::net::IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            !(ip.is_loopback()
                || ip.is_unspecified()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

fn attach_requested_url(fetched_results: &mut [SearchResult], requested_url: &str) {
    for fetched in fetched_results {
        fetched.url = requested_url.to_string();
    }
}

fn enrich_results_with_fetch(results: &mut Vec<SearchResult>, fetched_results: Vec<SearchResult>) {
    for fetched in fetched_results {
        if let Some(existing) = results
            .iter_mut()
            .find(|result| !fetched.url.is_empty() && result.url == fetched.url)
        {
            append_fetch_snippet(existing, &fetched.snippet);
        } else {
            results.push(fetched);
        }
    }
}

async fn enrich_github_results(
    stdin: &mut tokio::process::ChildStdin,
    stdout: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    request_id: &mut u64,
    results: &mut [SearchResult],
    top_n: usize,
) {
    let resources = github_resources(results, top_n);
    for (index, resource) in resources {
        if let Err(err) =
            write_json_line(stdin, &github_fetch_call_request(*request_id, &resource)).await
        {
            tracing::debug!(%err, "failed to write GitHub MCP fetch request");
            return;
        }
        *request_id += 1;

        let response = match read_json_line(stdout).await {
            Ok(response) => response,
            Err(err) => {
                tracing::debug!(%err, "failed to read GitHub MCP fetch response");
                return;
            }
        };

        if let Some(fetched_text) = extract_github_fetch_text(&response) {
            append_fetch_snippet(&mut results[index], &fetched_text);
        }
    }
}

fn github_resources(results: &[SearchResult], top_n: usize) -> Vec<(usize, GithubResource)> {
    let mut resources = Vec::new();
    for (index, result) in results.iter().enumerate() {
        let Some(resource) = parse_github_resource_url(&result.url) else {
            continue;
        };

        resources.push((index, resource));
        if resources.len() >= top_n {
            break;
        }
    }

    resources
}

fn append_fetch_snippet(result: &mut SearchResult, fetched_snippet: &str) {
    if fetched_snippet.trim().is_empty() {
        return;
    }

    let combined = format!(
        "{} Fetch: {}",
        result.snippet.trim(),
        fetched_snippet.trim()
    );
    result.snippet = truncate_chars(combined.trim(), MAX_RESULT_SNIPPET_CHARS);
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
    let trimmed = text.trim();
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return Ok(parse_text_results(source, trimmed));
    };

    let items = match value.as_array() {
        Some(items) => items,
        None => value
            .get("results")
            .or_else(|| value.get("items"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                anyhow::anyhow!("MCP tool output must be array or object with results/items")
            })?,
    };

    Ok(items
        .iter()
        .filter_map(|item| parse_result(source, item))
        .collect())
}

fn parse_text_results(source: SearchSource, text: &str) -> Vec<SearchResult> {
    text.split("\n---\n")
        .filter_map(|block| parse_text_result(source, block))
        .collect()
}

fn parse_text_result(source: SearchSource, block: &str) -> Option<SearchResult> {
    let mut title = String::new();
    let mut url = String::new();
    let mut snippet = String::new();
    let mut in_highlights = false;
    let mut can_collect_body = false;

    for line in block.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Title:") {
            title = value.trim().to_string();
            can_collect_body = true;
            in_highlights = false;
        } else if let Some(value) = line.strip_prefix("# ") {
            if title.is_empty() {
                title = value.trim().to_string();
            }
            can_collect_body = true;
            in_highlights = false;
        } else if let Some(value) = line.strip_prefix("URL:") {
            url = value.trim().to_string();
            can_collect_body = true;
            in_highlights = false;
        } else if let Some(value) = line.strip_prefix("Highlights:") {
            snippet = value.trim().to_string();
            in_highlights = true;
        } else if let Some(value) = line.strip_prefix("Text:") {
            snippet = value.trim().to_string();
            in_highlights = true;
        } else if let Some(value) = line.strip_prefix("Content:") {
            snippet = value.trim().to_string();
            in_highlights = true;
        } else if line.starts_with("Published:") || line.starts_with("Author:") {
            in_highlights = false;
        } else if line.ends_with(':') {
            in_highlights = false;
        } else if !line.is_empty() && (in_highlights || can_collect_body) {
            if !snippet.is_empty() {
                snippet.push(' ');
            }
            snippet.push_str(line);
        }
    }

    title = truncate_chars(title.trim(), MAX_RESULT_TITLE_CHARS);
    snippet = truncate_chars(snippet.trim(), MAX_RESULT_SNIPPET_CHARS);

    if title.is_empty() && snippet.is_empty() {
        return None;
    }

    Some(SearchResult {
        source,
        title,
        url,
        snippet,
    })
}

fn parse_result(source: SearchSource, item: &Value) -> Option<SearchResult> {
    let title = truncate_chars(
        first_text_field(item, &["title", "name", "full_name"]),
        MAX_RESULT_TITLE_CHARS,
    );
    let snippet = truncate_chars(
        first_text_field(item, &["snippet", "body", "description", "text", "content"]),
        MAX_RESULT_SNIPPET_CHARS,
    );

    if title.is_empty() && snippet.is_empty() {
        return None;
    }

    Some(SearchResult {
        source,
        title,
        url: first_text_field(item, &["html_url", "url"]).to_string(),
        snippet,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum GithubResource {
    Issue {
        owner: String,
        repo: String,
        issue_number: u64,
    },
    File {
        owner: String,
        repo: String,
        path: String,
        reference: String,
    },
}

fn parse_github_resource_url(url: &str) -> Option<GithubResource> {
    let path = url
        .trim()
        .strip_prefix("https://github.com/")
        .or_else(|| url.trim().strip_prefix("http://github.com/"))?;
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() < 4 {
        return None;
    }

    let owner = parts[0].to_string();
    let repo = parts[1].to_string();
    match parts[2] {
        "issues" | "pull" => {
            parts[3]
                .parse::<u64>()
                .ok()
                .map(|issue_number| GithubResource::Issue {
                    owner,
                    repo,
                    issue_number,
                })
        }
        "blob" if parts.len() >= 5 => Some(GithubResource::File {
            owner,
            repo,
            reference: parts[3].to_string(),
            path: parts[4..].join("/"),
        }),
        _ => None,
    }
}

fn extract_github_fetch_text(response: &Value) -> Option<String> {
    let mut texts = Vec::new();
    for item in response
        .pointer("/result/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(text) = item.get("text").and_then(Value::as_str) else {
            continue;
        };
        texts.push(extract_github_text_block(text));
    }

    let joined = texts
        .into_iter()
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!joined.trim().is_empty()).then(|| truncate_chars(joined.trim(), MAX_RESULT_SNIPPET_CHARS))
}

fn extract_github_text_block(text: &str) -> String {
    let trimmed = text.trim();
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return truncate_chars(trimmed, MAX_RESULT_SNIPPET_CHARS);
    };

    let mut fields = Vec::new();
    collect_github_text_fields(&value, &mut fields);
    fields
        .into_iter()
        .filter_map(normalize_github_text_field)
        .collect::<Vec<_>>()
        .join(" ")
}

fn collect_github_text_fields(value: &Value, fields: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "body" | "description" | "text" | "content") {
                    if let Some(text) = value.as_str() {
                        fields.push(text.to_string());
                    }
                }
                collect_github_text_fields(value, fields);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_github_text_fields(item, fields);
            }
        }
        _ => {}
    }
}

fn normalize_github_text_field(text: String) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    if looks_like_base64(trimmed) {
        if let Ok(decoded) = BASE64.decode(trimmed.replace('\n', "")) {
            if let Ok(decoded_text) = String::from_utf8(decoded) {
                return Some(truncate_chars(
                    decoded_text.trim(),
                    MAX_RESULT_SNIPPET_CHARS,
                ));
            }
        }
    }

    Some(truncate_chars(trimmed, MAX_RESULT_SNIPPET_CHARS))
}

fn looks_like_base64(text: &str) -> bool {
    text.len() >= 40
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '\n' | '\r'))
}

fn first_text_field<'a>(item: &'a Value, fields: &[&str]) -> &'a str {
    for field in fields {
        if let Some(value) = item.get(*field).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
    }

    if fields.contains(&"full_name") {
        if let Some(value) = item
            .get("repository")
            .and_then(|repository| repository.get("full_name"))
            .and_then(Value::as_str)
        {
            return value.trim();
        }
    }

    ""
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_url_filter_rejects_private_and_non_http_targets() {
        for url in [
            "http://127.0.0.1/admin",
            "http://10.0.0.1/",
            "http://169.254.169.254/latest/meta-data",
            "http://localhost:8080/",
            "file:///etc/passwd",
            "https://user:pass@example.com/",
        ] {
            assert!(!is_safe_fetch_url(url), "must reject {url}");
        }
    }

    #[test]
    fn fetch_url_filter_accepts_public_http_targets() {
        assert!(is_safe_fetch_url("https://example.com/news"));
        assert!(is_safe_fetch_url("http://93.184.216.34/news"));
    }

    #[test]
    fn exa_tool_uses_num_results_argument() {
        let query = SearchQuery {
            source: SearchSource::Web,
            text: "AYANEO NEXT 2".to_string(),
        };

        assert_eq!(
            tool_arguments("web_search_exa", &query),
            json!({"query":"AYANEO NEXT 2","numResults":5})
        );
    }

    #[test]
    fn github_repository_tool_uses_per_page_argument() {
        let query = SearchQuery {
            source: SearchSource::Github,
            text: "tokio release".to_string(),
        };

        assert_eq!(
            tool_arguments("search_repositories", &query),
            json!({"query":"tokio release","perPage":5})
        );
    }

    #[test]
    fn github_issue_tool_uses_q_argument() {
        let query = SearchQuery {
            source: SearchSource::Github,
            text: "ripgrep release changelog".to_string(),
        };

        assert_eq!(
            tool_arguments("search_issues", &query),
            json!({"q":"ripgrep release changelog","per_page":5})
        );
    }

    #[test]
    fn github_tool_names_keep_only_readonly_search_tools() {
        let mut config = Config::from_env();
        config.search_github_mcp_tools = vec![
            "search_issues".to_string(),
            "create_issue".to_string(),
            "search_code".to_string(),
            "push_files".to_string(),
        ];

        assert_eq!(
            github_tool_names(&config),
            vec!["search_issues".to_string(), "search_code".to_string()]
        );
    }

    #[test]
    fn parses_github_issue_url() {
        assert_eq!(
            parse_github_resource_url("https://github.com/BurntSushi/ripgrep/issues/2658"),
            Some(GithubResource::Issue {
                owner: "BurntSushi".to_string(),
                repo: "ripgrep".to_string(),
                issue_number: 2658,
            })
        );
    }

    #[test]
    fn parses_github_pull_url_as_issue_resource() {
        assert_eq!(
            parse_github_resource_url("https://github.com/BurntSushi/ripgrep/pull/3456"),
            Some(GithubResource::Issue {
                owner: "BurntSushi".to_string(),
                repo: "ripgrep".to_string(),
                issue_number: 3456,
            })
        );
    }

    #[test]
    fn parses_github_blob_url() {
        assert_eq!(
            parse_github_resource_url(
                "https://github.com/BurntSushi/ripgrep/blob/master/CHANGELOG.md"
            ),
            Some(GithubResource::File {
                owner: "BurntSushi".to_string(),
                repo: "ripgrep".to_string(),
                reference: "master".to_string(),
                path: "CHANGELOG.md".to_string(),
            })
        );
    }

    #[test]
    fn extracts_github_fetch_text_from_json_content() {
        let response = json!({
            "result": {
                "content": [{
                    "type": "text",
                    "text": r#"{"content":"IyBDaGFuZ2Vsb2cKCi0gUmVsZWFzZSBub3RlcyBmb3IgdjE1"}"#
                }]
            }
        });

        let text = extract_github_fetch_text(&response).unwrap();

        assert!(text.contains("# Changelog"));
        assert!(text.contains("Release notes"));
    }

    #[test]
    fn github_resources_keeps_top_n_parseable_urls() {
        let results = vec![
            SearchResult {
                source: SearchSource::Github,
                title: "CHANGELOG.md".to_string(),
                url: "https://github.com/org/repo/blob/main/CHANGELOG.md".to_string(),
                snippet: String::new(),
            },
            SearchResult {
                source: SearchSource::Github,
                title: "Issue".to_string(),
                url: "https://github.com/org/repo/issues/7".to_string(),
                snippet: String::new(),
            },
        ];

        assert_eq!(github_resources(&results, 1).len(), 1);
        assert_eq!(github_resources(&results, 2).len(), 2);
    }

    #[test]
    fn exa_fetch_uses_urls_and_max_characters() {
        let urls = vec!["https://example.com/a".to_string()];

        assert_eq!(
            fetch_tool_arguments("web_fetch_exa", &urls, 6000),
            json!({"urls":["https://example.com/a"],"maxCharacters":6000})
        );
    }

    #[test]
    fn enriches_existing_result_with_fetched_text() {
        let mut results = vec![SearchResult {
            source: SearchSource::Web,
            title: "AYANEO NEXT 2".to_string(),
            url: "https://example.com/ayaneo".to_string(),
            snippet: "Search snippet.".to_string(),
        }];
        let fetched = vec![SearchResult {
            source: SearchSource::Web,
            title: "AYANEO NEXT 2".to_string(),
            url: "https://example.com/ayaneo".to_string(),
            snippet: "Fetched page confirms $5299 tier.".to_string(),
        }];

        enrich_results_with_fetch(&mut results, fetched);

        assert_eq!(results.len(), 1);
        assert!(results[0].snippet.contains("Search snippet."));
        assert!(
            results[0]
                .snippet
                .contains("Fetched page confirms $5299 tier.")
        );
    }

    #[test]
    fn parses_exa_fetch_text_results() {
        let output = "# AYANEO NEXT 2\nURL: https://shop.ayaneo.com/products/ayaneo-next-2\nPublished: 2026-06-15\n\n$3,699.00\n\nSeries AI385-32GB+1TB-Polar Black AI395-64GB+1TB-Polar Black AI395-128GB+2TB-Polar Black\n\nAI395-128GB+2TB-Polar Black - Sold Out";

        let results = parse_tool_output(SearchSource::Web, output).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].url,
            "https://shop.ayaneo.com/products/ayaneo-next-2"
        );
        assert!(results[0].snippet.contains("AI395-128GB+2TB-Polar Black"));
    }

    #[test]
    fn parses_exa_text_results() {
        let output = "Title: AYANEO NEXT 2 Strix Halo handheld starts global shipping\nURL: https://videocardz.com/newz/ayaneo-next-2\nPublished: 2026-07-04T11:46:38.000Z\nAuthor: WhyCry\nHighlights:\nRyzen AI Max 385 with 32GB of memory and 1TB of storage: $2999.\n\n---\n\nTitle: AYANEO NEXT 2\nURL: https://shop.ayaneo.com/products/ayaneo-next-2\nHighlights:\nAI395-128GB+2TB-Polar Black - Sold Out";

        let results = parse_tool_output(SearchSource::Web, output).unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].title,
            "AYANEO NEXT 2 Strix Halo handheld starts global shipping"
        );
        assert_eq!(results[0].url, "https://videocardz.com/newz/ayaneo-next-2");
        assert!(results[0].snippet.contains("$2999"));
        assert_eq!(results[1].title, "AYANEO NEXT 2");
    }

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
    fn parses_github_items_results() {
        let output = r#"{
            "total_count": 1,
            "items": [
                {
                    "html_url": "https://github.com/BurntSushi/ripgrep/issues/2658",
                    "title": "Release 15.0.0 changelog",
                    "body": "Tracking issue for the release notes and changelog.",
                    "state": "closed",
                    "repository": { "full_name": "BurntSushi/ripgrep" }
                }
            ],
            "search_type": "lexical"
        }"#;

        let results = parse_tool_output(SearchSource::Github, output).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, SearchSource::Github);
        assert_eq!(results[0].title, "Release 15.0.0 changelog");
        assert_eq!(
            results[0].url,
            "https://github.com/BurntSushi/ripgrep/issues/2658"
        );
        assert!(results[0].snippet.contains("release notes"));
    }

    #[test]
    fn parses_github_code_items_with_repository_title_fallback() {
        let output = r#"{
            "items": [
                {
                    "html_url": "https://github.com/org/repo/blob/main/CHANGELOG.md",
                    "text_matches": [],
                    "repository": { "full_name": "org/repo" }
                }
            ]
        }"#;

        let results = parse_tool_output(SearchSource::Github, output).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "org/repo");
        assert_eq!(
            results[0].url,
            "https://github.com/org/repo/blob/main/CHANGELOG.md"
        );
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
