use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::config::Config;
use crate::features::memory::service::MemoryNote;
use crate::features::search::extract::{extract_research_plan, sanitize_external_queries};
use crate::features::search::mcp::McpSearchProvider;
use crate::features::search::policy::is_allowed_search_result;
use crate::features::search::provider::SearchProvider;
use crate::features::search::types::{
    MAX_SEARCH_RESULTS, SearchContext, SearchQuery, SearchResult,
};

pub async fn run_search(
    config: &Config,
    clean_post: &str,
    memory_notes: &[MemoryNote],
) -> SearchContext {
    let started = Instant::now();

    if !config.search_enabled {
        return SearchContext::skipped("disabled", started.elapsed().as_millis());
    }

    run_search_enabled(config, clean_post, memory_notes, started).await
}

async fn run_search_enabled(
    config: &Config,
    clean_post: &str,
    memory_notes: &[MemoryNote],
    started: Instant,
) -> SearchContext {
    let plan = match extract_research_plan(config, clean_post, memory_notes).await {
        Ok(plan) => plan,
        Err(err) => {
            tracing::warn!(%err, "failed to extract search queries");
            return SearchContext::skipped("extract_failed", started.elapsed().as_millis());
        }
    };

    let queries = sanitize_external_queries(&plan);
    if queries.is_empty() {
        return SearchContext {
            plan: Some(plan),
            queries,
            results: Vec::new(),
            skipped_reason: Some("no_search_needed".to_string()),
            latency_ms: started.elapsed().as_millis(),
        };
    }

    let results = run_queries_in_parallel(config, &queries).await;

    let results = dedupe_results(
        results
            .into_iter()
            .filter(|result| is_allowed_search_result(config, result))
            .collect(),
    );
    if results.is_empty() {
        return SearchContext {
            plan: Some(plan),
            queries,
            results,
            skipped_reason: Some("no_results".to_string()),
            latency_ms: started.elapsed().as_millis(),
        };
    }

    SearchContext {
        plan: Some(plan),
        queries,
        results,
        skipped_reason: None,
        latency_ms: started.elapsed().as_millis(),
    }
}

async fn run_queries_in_parallel(config: &Config, queries: &[SearchQuery]) -> Vec<SearchResult> {
    let mut tasks = JoinSet::new();
    // Web and Reddit share the same remote Exa MCP. Starting every query at once
    // made those short-lived proxy processes compete with each other and lose
    // otherwise valid search responses. GitHub uses a separate MCP process.
    let shared_mcp_slots = Arc::new(Semaphore::new(2));

    for (index, query) in queries.iter().cloned().enumerate() {
        let config = config.clone();
        let shared_mcp_slots = Arc::clone(&shared_mcp_slots);
        tasks.spawn(async move {
            let source = query.source;
            let _shared_mcp_permit = if uses_shared_mcp(&config, source) {
                Some(
                    shared_mcp_slots
                        .acquire_owned()
                        .await
                        .expect("shared MCP semaphore is never closed"),
                )
            } else {
                None
            };
            let provider = McpSearchProvider::new(config);
            (index, source, provider.search(&query).await)
        });
    }

    let mut results_by_query = vec![Vec::new(); queries.len()];
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((index, _, Ok(results))) => results_by_query[index] = results,
            Ok((_, source, Err(err))) => {
                tracing::warn!(%err, ?source, "search provider failed")
            }
            Err(err) => tracing::warn!(%err, "search task failed"),
        }
    }

    results_by_query.into_iter().flatten().collect()
}

fn uses_shared_mcp(config: &Config, source: crate::features::search::types::SearchSource) -> bool {
    source != crate::features::search::types::SearchSource::Github
        || config.search_github_mcp_command.is_none()
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

        let context = run_search(&config, "post", &[]).await;

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

        let context = run_search(&config, "post", &[]).await;

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
