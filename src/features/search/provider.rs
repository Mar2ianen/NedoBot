use crate::features::search::types::{SearchQuery, SearchResult};

#[async_trait::async_trait]
#[allow(clippy::double_must_use)]
pub trait SearchProvider: Send + Sync {
    async fn search(&self, query: &SearchQuery) -> anyhow::Result<Vec<SearchResult>>;
}
