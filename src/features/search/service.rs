use std::collections::HashSet;
use std::time::{Duration, Instant};

use tokio::time::timeout;

use crate::config::Config;
use crate::features::search::extract::extract_search_queries;
use crate::features::search::mcp::McpSearchProvider;
use crate::features::search::provider::SearchProvider;
use crate::features::search::types::{MAX_SEARCH_RESULTS, SearchContext, SearchResult};

pub async fn run_search(config: &Config, clean_post: &str) -> SearchContext {
    let started = Instant::now();

    if !config.search_enabled {
        return SearchContext::skipped("disabled", started.elapsed().as_millis());
    }

    let timeout_duration = Duration::from_secs(config.search_mcp_timeout_sec);
    match timeout(
        timeout_duration,
        run_search_enabled(config, clean_post, started),
    )
    .await
    {
        Ok(context) => context,
        Err(_) => {
            tracing::warn!("search run timed out");
            SearchContext::skipped("timeout", started.elapsed().as_millis())
        }
    }
}

async fn run_search_enabled(config: &Config, clean_post: &str, started: Instant) -> SearchContext {
    let queries = match extract_search_queries(config, clean_post).await {
        Ok(queries) => queries,
        Err(err) => {
            tracing::warn!(%err, "failed to extract search queries");
            return SearchContext::skipped("extract_failed", started.elapsed().as_millis());
        }
    };

    if queries.is_empty() {
        return SearchContext::skipped("no_search_needed", started.elapsed().as_millis());
    }

    let provider = McpSearchProvider::new(config.clone());
    let mut results = Vec::new();

    for query in &queries {
        match provider.search(query).await {
            Ok(query_results) => results.extend(query_results),
            Err(err) => tracing::warn!(%err, source = ?query.source, "search provider failed"),
        }
    }

    let results = dedupe_results(results);
    if results.is_empty() {
        return SearchContext {
            queries,
            results,
            skipped_reason: Some("no_results".to_string()),
            latency_ms: started.elapsed().as_millis(),
        };
    }

    SearchContext {
        queries,
        results,
        skipped_reason: None,
        latency_ms: started.elapsed().as_millis(),
    }
}

fn dedupe_results(results: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();

    for result in results {
        let key = dedupe_key(&result);
        if !seen.insert(key) {
            continue;
        }

        deduped.push(result);
        if deduped.len() >= MAX_SEARCH_RESULTS {
            break;
        }
    }

    deduped
}

fn dedupe_key(result: &SearchResult) -> String {
    if !result.url.trim().is_empty() {
        return format!("url:{}", result.url.trim());
    }

    format!(
        "text:{}\n{}",
        result.title.trim().to_lowercase(),
        result.snippet.trim().to_lowercase()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::search::types::SearchSource;

    #[tokio::test]
    async fn disabled_returns_skipped_disabled() {
        let mut config = Config::from_env();
        config.search_enabled = false;

        let context = run_search(&config, "post").await;

        assert_eq!(context.skipped_reason.as_deref(), Some("disabled"));
        assert!(context.queries.is_empty());
        assert!(context.results.is_empty());
    }

    #[tokio::test]
    async fn extract_error_returns_skipped_extract_failed() {
        let mut config = Config::from_env();
        config.search_enabled = true;
        config.search_extract_provider = Some("unsupported".to_string());
        config.search_mcp_command = Some("unused".to_string());
        config.search_mcp_timeout_sec = 1;

        let context = run_search(&config, "post").await;

        assert_eq!(context.skipped_reason.as_deref(), Some("extract_failed"));
        assert!(context.queries.is_empty());
        assert!(context.results.is_empty());
    }

    #[test]
    fn dedupes_by_url() {
        let results = dedupe_results(vec![
            result("One", "https://example.com/a", "first"),
            result("Duplicate title", "https://example.com/a", "second"),
        ]);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "One");
    }

    #[test]
    fn dedupes_empty_url_by_text() {
        let results = dedupe_results(vec![
            result("Same", "", "Snippet"),
            result("same", "", "snippet"),
            result("Same", "", "Different"),
        ]);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].snippet, "Snippet");
        assert_eq!(results[1].snippet, "Different");
    }

    #[test]
    fn keeps_different_urls() {
        let results = dedupe_results(vec![
            result("Same", "https://example.com/a", "Snippet"),
            result("Same", "https://example.com/b", "Snippet"),
        ]);

        assert_eq!(results.len(), 2);
    }

    fn result(title: &str, url: &str, snippet: &str) -> SearchResult {
        SearchResult {
            source: SearchSource::Web,
            title: title.to_string(),
            url: url.to_string(),
            snippet: snippet.to_string(),
        }
    }
}
