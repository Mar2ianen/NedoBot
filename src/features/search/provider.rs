use crate::features::search::types::{SearchQuery, SearchResult};

#[async_trait::async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search(&self, query: &SearchQuery) -> anyhow::Result<Vec<SearchResult>>;
}
