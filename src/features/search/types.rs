#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SearchSource {
    Web,
    Github,
    Reddit,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SearchQuery {
    pub source: SearchSource,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SearchResult {
    pub source: SearchSource,
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchContext {
    pub queries: Vec<SearchQuery>,
    pub results: Vec<SearchResult>,
    pub skipped_reason: Option<String>,
    pub latency_ms: u128,
}

impl SearchContext {
    pub fn skipped(reason: impl Into<String>, latency_ms: u128) -> Self {
        Self {
            queries: Vec::new(),
            results: Vec::new(),
            skipped_reason: Some(reason.into()),
            latency_ms,
        }
    }

    pub fn is_skipped(&self) -> bool {
        self.skipped_reason.is_some()
    }
}

pub const MAX_SEARCH_QUERIES: usize = 3;
pub const MAX_QUERY_CHARS: usize = 180;
pub const MAX_SEARCH_RESULTS: usize = 8;
pub const MAX_RESULT_TITLE_CHARS: usize = 140;
pub const MAX_RESULT_SNIPPET_CHARS: usize = 6000;
