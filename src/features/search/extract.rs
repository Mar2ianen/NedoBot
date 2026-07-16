use std::collections::HashSet;

use serde::Deserialize;

use crate::config::Config;
use crate::features::search::types::{
    MAX_QUERY_CHARS, MAX_SEARCH_QUERIES, SearchQuery, SearchSource,
};
use crate::llm::service::generate_text_with_provider;

const SEARCH_EXTRACT_PROMPT: &str = include_str!("../../../prompts/search_extract.md");

#[derive(Debug, Deserialize)]
struct ExtractResponse {
    need_search: bool,
    #[serde(default)]
    queries: Vec<ExtractQuery>,
}

#[derive(Debug, Deserialize)]
struct ExtractQuery {
    source: String,
    text: String,
}

pub async fn extract_search_queries(
    config: &Config,
    clean_post: &str,
) -> anyhow::Result<Vec<SearchQuery>> {
    let prompt = format!("{SEARCH_EXTRACT_PROMPT}\n\nPOST:\n{clean_post}");
    let response = generate_text_with_provider(
        config,
        config.search_extract_provider.as_deref(),
        config.search_extract_model.as_deref(),
        &prompt,
        None,
        config.search_extract_temperature,
        config.search_extract_max_tokens,
    )
    .await?;

    parse_extract_response(&response.content)
}

fn parse_extract_response(value: &str) -> anyhow::Result<Vec<SearchQuery>> {
    let response: ExtractResponse = serde_json::from_str(strip_json_fence(value))?;

    if !response.need_search {
        return Ok(Vec::new());
    }

    Ok(sanitize_queries(response.queries))
}

fn strip_json_fence(value: &str) -> &str {
    let trimmed = value.trim();
    let Some(without_opening) = trimmed.strip_prefix("```") else {
        return trimmed;
    };

    let without_language = without_opening
        .strip_prefix("json")
        .or_else(|| without_opening.strip_prefix("JSON"))
        .unwrap_or(without_opening)
        .trim_start();

    without_language
        .strip_suffix("```")
        .map(str::trim)
        .unwrap_or(trimmed)
}

fn sanitize_queries(queries: Vec<ExtractQuery>) -> Vec<SearchQuery> {
    let mut seen = HashSet::new();
    let mut sanitized = Vec::new();

    for query in queries {
        let text = truncate_chars(query.text.trim(), MAX_QUERY_CHARS);
        if text.is_empty() {
            continue;
        }

        let Some(source) = parse_source(&query.source) else {
            continue;
        };

        let dedupe_key = (source, text.to_lowercase());
        if !seen.insert(dedupe_key) {
            continue;
        }

        sanitized.push(SearchQuery { source, text });

        if sanitized.len() >= MAX_SEARCH_QUERIES {
            break;
        }
    }

    sanitized
}

fn parse_source(source: &str) -> Option<SearchSource> {
    match source.trim().to_lowercase().as_str() {
        "web" => Some(SearchSource::Web),
        "github" => Some(SearchSource::Github),
        "reddit" => Some(SearchSource::Reddit),
        _ => None,
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_json() {
        let queries = parse_extract_response(
            r#"{
                "need_search": true,
                "queries": [
                    { "source": "web", "text": "Rust 1.90 release notes" },
                    { "source": "github", "text": "tokio changelog" }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0].source, SearchSource::Web);
        assert_eq!(queries[0].text, "Rust 1.90 release notes");
        assert_eq!(queries[1].source, SearchSource::Github);
    }

    #[test]
    fn parses_fenced_json() {
        let queries = parse_extract_response(
            r#"```json
            {
                "need_search": true,
                "queries": [
                    { "source": "reddit", "text": "RTX 5090 reddit benchmark" }
                ]
            }
            ```"#,
        )
        .unwrap();

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].source, SearchSource::Reddit);
        assert_eq!(queries[0].text, "RTX 5090 reddit benchmark");
    }

    #[test]
    fn no_search_returns_empty_queries() {
        let queries = parse_extract_response(r#"{"need_search":false,"queries":[]}"#).unwrap();

        assert!(queries.is_empty());
    }

    #[test]
    fn drops_duplicate_queries() {
        let queries = parse_extract_response(
            r#"{
                "need_search": true,
                "queries": [
                    { "source": "web", "text": "  Gemini release date  " },
                    { "source": "web", "text": "gemini release date" },
                    { "source": "github", "text": "gemini release date" }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0].source, SearchSource::Web);
        assert_eq!(queries[1].source, SearchSource::Github);
    }

    #[test]
    fn keeps_four_distinct_queries() {
        let queries = parse_extract_response(
            r#"{
                "need_search":true,
                "queries":[
                    {"source":"web","text":"product alternatives"},
                    {"source":"web","text":"product ownership impact"},
                    {"source":"github","text":"product compatibility"},
                    {"source":"reddit","text":"product community experience"}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(queries.len(), MAX_SEARCH_QUERIES);
    }

    #[test]
    fn drops_unknown_source() {
        let queries = parse_extract_response(
            r#"{
                "need_search": true,
                "queries": [
                    { "source": "telegram", "text": "ignored" },
                    { "source": "web", "text": "kept" }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].text, "kept");
    }

    #[test]
    fn truncates_long_query() {
        let long_query = "a".repeat(MAX_QUERY_CHARS + 20);
        let response = format!(
            r#"{{"need_search":true,"queries":[{{"source":"web","text":"{long_query}"}}]}}"#
        );

        let queries = parse_extract_response(&response).unwrap();

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].text.chars().count(), MAX_QUERY_CHARS);
    }
}
