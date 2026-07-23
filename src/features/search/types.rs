#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SearchSource {
    Web,
    Github,
    Reddit,
}

impl SearchSource {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Web => "веб-поиск",
            Self::Github => "GitHub",
            Self::Reddit => "Reddit",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SearchQuery {
    pub source: SearchSource,
    pub text: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ResearchPlan {
    pub primary_subject: String,
    #[serde(default)]
    pub primary_audience: Vec<String>,
    #[serde(default)]
    pub secondary_context: Vec<String>,
    #[serde(default)]
    pub chat_semantic_queries: Vec<String>,
    #[serde(default)]
    pub chat_lexical_terms: Vec<String>,
    #[serde(default)]
    pub web_queries: Vec<String>,
    #[serde(default)]
    pub reddit_queries: Vec<String>,
    #[serde(default)]
    pub github_queries: Vec<String>,
}

impl ResearchPlan {
    pub fn external_queries(&self) -> Vec<SearchQuery> {
        let mut queries = Vec::new();
        for (source, source_queries) in [
            (SearchSource::Web, &self.web_queries),
            (SearchSource::Reddit, &self.reddit_queries),
            (SearchSource::Github, &self.github_queries),
        ] {
            for text in source_queries {
                queries.push(SearchQuery {
                    source,
                    text: text.clone(),
                });
            }
        }
        queries
    }
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
    pub plan: Option<ResearchPlan>,
    pub queries: Vec<SearchQuery>,
    pub results: Vec<SearchResult>,
    pub skipped_reason: Option<String>,
    pub latency_ms: u128,
}

impl SearchContext {
    pub fn skipped(reason: impl Into<String>, latency_ms: u128) -> Self {
        Self {
            plan: None,
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

pub const MAX_SEARCH_QUERIES: usize = 4;
pub const MAX_QUERY_CHARS: usize = 180;
pub const MAX_SEARCH_RESULTS: usize = 24;
pub const MAX_RESULT_TITLE_CHARS: usize = 180;
pub const MAX_RESULT_SNIPPET_CHARS: usize = 16_000;
