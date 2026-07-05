# План: провайдер-нейтральный поиск фактов для первого комментария

Цель: добавить optional web/GitHub/Reddit факт-чек перед генерацией первого комментария, не меняя текущее поведение при `SEARCH_ENABLED=false`.

## Финальная архитектура первой итерации

```text
пост канала
  → search extract LLM: определить, нужен ли поиск, и вернуть JSON с запросами
  → lazy MCP process: запустить SEARCH_MCP_COMMAND на один search-run
  → MCP tools/call: выполнить web/github/reddit запросы
  → короткий SearchContext: queries + results + skipped_reason + latency
  → build_llm_prompt: добавить блок свежего поиска как вспомогательный контекст
  → generate_text_checked через текущий LLM_PROVIDER
  → validate_comment_output без изменений
  → build_comment_html без изменений
```

Ключевое решение: **MCP process запускается лениво на каждый search-run**. В первой итерации не держим long-lived child process в `AppState`, не реализуем restart/shutdown lifecycle и не добавляем DB cache.

## Инвариант поведения

- `SEARCH_ENABLED=false` — поведение строго идентично текущему.
- Любая ошибка extract/MCP/search parsing/timeout → `SearchContext::skipped(...)`, комментарий генерируется как сейчас.
- Search не должен блокировать first-comment pipeline дольше `SEARCH_MCP_TIMEOUT_SEC`.
- Search facts имеют приоритет ниже поста, `tech_rag` и валидатора.
- Search не меняет `validate_comment_output`.
- Search не добавляется в voice cleanup.

## Конфиг

Добавить в `Config`:

```rust
pub search_enabled: bool,
pub search_extract_provider: Option<String>,
pub search_extract_model: Option<String>,
pub search_extract_temperature: f32,
pub search_extract_max_tokens: u32,
pub search_mcp_command: Option<String>,
pub search_mcp_args: Vec<String>,
pub search_mcp_env: Vec<String>,
pub search_mcp_timeout_sec: u64,
pub search_mcp_tools: SearchMcpTools,
```

Добавить helper struct:

```rust
#[derive(Clone)]
pub struct SearchMcpTools {
    pub web: String,
    pub github: String,
    pub reddit: String,
}
```

Env defaults:

```env
SEARCH_ENABLED=false
SEARCH_EXTRACT_PROVIDER=ollama
SEARCH_EXTRACT_MODEL=gemma4:31b
SEARCH_EXTRACT_TEMPERATURE=0.1
SEARCH_EXTRACT_MAX_TOKENS=700
SEARCH_MCP_COMMAND=
SEARCH_MCP_ARGS=
SEARCH_MCP_ENV=
SEARCH_MCP_TIMEOUT_SEC=8
SEARCH_MCP_TOOL_WEB=web_search
SEARCH_MCP_TOOL_GITHUB=github_search
SEARCH_MCP_TOOL_REDDIT=reddit_search
```

Parsing rules:

- `SEARCH_MCP_ARGS` — split whitespace, empty → `Vec::new()`.
- `SEARCH_MCP_ENV` — comma-separated list of env variable names allowed to pass from bot process to MCP child.
- `SEARCH_MCP_COMMAND` is required only when `SEARCH_ENABLED=true`.
- `SEARCH_EXTRACT_PROVIDER/MODEL` are required only when `SEARCH_ENABLED=true`.

`validate_runtime_secrets`:

```rust
if self.search_enabled {
    validate_search_config(&mut errors, self);
    validate_llm_provider_secret(... SEARCH_EXTRACT_PROVIDER ...);
    validate_llm_provider_model(... SEARCH_EXTRACT_PROVIDER ...);
}
```

Важно: validation строго gated by `search_enabled`.

## Новые файлы

```text
src/features/search/mod.rs
src/features/search/types.rs
src/features/search/extract.rs
src/features/search/provider.rs
src/features/search/mcp.rs
src/features/search/service.rs
prompts/search_extract.md
```

В `src/features/mod.rs` добавить:

```rust
pub mod search;
```

## Search types

`src/features/search/types.rs`:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSource {
    Web,
    Github,
    Reddit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchQuery {
    pub source: SearchSource,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
```

Required impl:

```rust
impl SearchContext {
    pub fn skipped(reason: impl Into<String>, latency_ms: u128) -> Self;
    pub fn is_skipped(&self) -> bool;
}
```

Limits:

```rust
pub const MAX_SEARCH_QUERIES: usize = 3;
pub const MAX_QUERY_CHARS: usize = 180;
pub const MAX_SEARCH_RESULTS: usize = 8;
pub const MAX_RESULT_TITLE_CHARS: usize = 140;
pub const MAX_RESULT_SNIPPET_CHARS: usize = 220;
```

## Extract prompt

`prompts/search_extract.md` должен требовать только JSON без markdown:

```text
Ты выбираешь поисковые запросы для факт-чека техно-новости.

Верни строго JSON без markdown и без пояснений:
{
  "need_search": true,
  "queries": [
    { "source": "web", "text": "..." }
  ]
}

Правила:
- Если в посте нет проверяемых дат, цифр, релизов, цен, прогнозов, названий моделей или спорных фактов, верни {"need_search":false,"queries":[]}.
- Максимум 3 запроса.
- source только: web, github, reddit.
- web: новости, аналитика, первоисточники, версии, цены, прогнозы.
- github: релизы, changelog, issues, benchmarks из репозиториев.
- reddit: реакция сообщества, если это полезно для контекста.
- Запросы пиши на английском, если тема международная.
- Не добавляй факты от себя.
```

`src/features/search/extract.rs`:

- build prompt: `SEARCH_EXTRACT_PROMPT + "\n\nPOST:\n" + clean_post`.
- call:

```rust
generate_text_with_provider(
    config,
    config.search_extract_provider.as_deref(),
    config.search_extract_model.as_deref(),
    &prompt,
    None,
    config.search_extract_temperature,
    config.search_extract_max_tokens,
)
```

- parse JSON via `serde_json`.
- accept optional fenced JSON by stripping ```json fences.
- if parse fails → return `Vec::new()` plus skipped reason from service.
- sanitize queries:
  - trim
  - drop empty
  - truncate to `MAX_QUERY_CHARS`
  - drop unknown source
  - dedupe by `(source, lowercase text)`
  - take `MAX_SEARCH_QUERIES`

## SearchProvider

`src/features/search/provider.rs`:

```rust
#[async_trait::async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search(&self, query: &SearchQuery) -> anyhow::Result<Vec<SearchResult>>;
}
```

## Lazy MCP provider

`src/features/search/mcp.rs` implements `McpSearchProvider`.

Behavior:

- For each call to `search(&SearchQuery)`:
  1. spawn `SEARCH_MCP_COMMAND` with `SEARCH_MCP_ARGS`.
  2. pass only env vars listed in `SEARCH_MCP_ENV`.
  3. initialize MCP stdio client.
  4. call one tool based on `SearchQuery.source`:
     - `Web` → `config.search_mcp_tools.web`
     - `Github` → `config.search_mcp_tools.github`
     - `Reddit` → `config.search_mcp_tools.reddit`
  5. parse tool result into `Vec<SearchResult>`.
  6. close process.
- Wrap the whole operation with `tokio::time::timeout(Duration::from_secs(config.search_mcp_timeout_sec), ...)`.
- On timeout/tool error/process error/parsing error return `Ok(Vec::new())` and log `tracing::warn!` without secrets.
- Do not log `SEARCH_MCP_ENV` values.

Tool input JSON:

```json
{ "query": "...", "limit": 5 }
```

Accepted tool output shapes:

```json
[
  { "title": "...", "url": "...", "snippet": "..." }
]
```

or:

```json
{
  "results": [
    { "title": "...", "url": "...", "snippet": "..." }
  ]
}
```

Normalize results:

- empty title and empty snippet → drop
- truncate title/snippet
- take remaining up to `MAX_SEARCH_RESULTS` at service level

## Search service

`src/features/search/service.rs`:

```rust
pub async fn run_search(config: &Config, clean_post: &str) -> SearchContext
```

Flow:

1. `let started = Instant::now();`
2. if `!config.search_enabled` → `SearchContext::skipped("disabled", latency)`.
3. run extract.
4. if extract failed → `skipped("extract_failed", latency)`.
5. if no queries → `skipped("no_search_needed", latency)`.
6. create `McpSearchProvider::new(config.clone())`.
7. for each query sequentially call provider.search(query).
8. collect results, dedupe by URL, take `MAX_SEARCH_RESULTS`.
9. if no results → context with queries, empty results, `skipped_reason=Some("no_results")`.
10. else return context with results.

First iteration intentionally sequential. Do not use `tokio::join!` yet.

## Prompt integration

Change `src/features/first_comment/prompt.rs`:

```rust
pub fn build_llm_prompt(
    post_text: &str,
    chat_member_count: Option<u32>,
    memory_notes: &[MemoryNote],
    recent_comments: &[String],
    search_context: Option<&SearchContext>,
) -> String
```

Insert search block after `tech_rag` and before memory:

```text
Свежий поиск, использовать осторожно:
- Это вспомогательный контекст, он ниже поста по приоритету.
- Не цитируй URL и не добавляй ссылки.
- Если поиск противоречит посту, не утверждай спорное как факт.
- Если результаты нерелевантны, игнорируй их.

Результаты:
- [web] Title — snippet
- [github] Title — snippet
```

If disabled/skipped/no results, include one short line:

```text
Свежий поиск: нет дополнительного контекста.
```

Do not put raw URLs into the prompt. If source identity is useful, include domain only later in a separate iteration.

## First-comment pipeline integration

Change `src/features/first_comment/pipeline.rs`:

Current order remains mostly sequential:

1. download image
2. get chat member count
3. load memory notes
4. load recent comments
5. `let search_context = run_search(config, &clean_post).await;`
6. build prompt with `Some(&search_context)` if `config.search_enabled`, else `None`
7. generate comment as before

Owner preview:

Change signature:

```rust
send_owner_preview(bot, owner_id, &final_html, candidate.source_message_id, &search_context).await;
```

Append one line:

```text
search=2 queries, 6 results, 1420ms
```

or:

```text
search=skipped(no_search_needed), 120ms
```

## Tests

Add unit tests:

- `extract.rs`
  - parses valid JSON
  - parses fenced JSON
  - returns no queries for invalid JSON
  - drops unknown source
  - dedupes duplicate queries
  - truncates long query
- `mcp.rs`
  - parses array tool output
  - parses `{ results: [...] }` tool output
  - drops empty result
  - truncates title/snippet
- `service.rs`
  - disabled returns skipped disabled
  - no queries returns skipped no_search_needed
  - result dedupe by URL
- `prompt.rs`
  - prompt includes search facts when context has results
  - prompt does not include raw URLs
  - prompt says no additional context when skipped

## Cargo.toml

Add dependencies/features needed by implementation:

```toml
async-trait = "0.1" # already present
```

For lazy process implementation, ensure tokio features include:

```toml
tokio = { version = "1", features = ["macros", "rt-multi-thread", "fs", "io-util", "process", "time"] }
```

MCP crate:

- Add `mcpr` only after checking its current API locally.
- If `mcpr` API does not support stdio client cleanly, implement minimal JSON-RPC stdio client in `src/features/search/mcp.rs` using `tokio::process::Command`.
- This decision must be made by the main agent before spawning implementation subagents.

## Documentation updates

- `.env.example`: add SEARCH section.
- `docs/TECHNICAL.md`: describe search pipeline and env vars.
- `docs/SEARCH_TODO.md`: keep implementation checklist.
- `docs/SEARCH_SUBAGENT_PROMPTS.md`: keep exact subagent prompts.

`docs/LOCAL_WORKFLOW.md` currently does not exist. Do not reference it unless created in a separate docs task.

## Not in first iteration

- long-lived MCP process in `AppState`
- restart/shutdown lifecycle
- DB cache `post_search_cache`
- migrations
- pgvector/embeddings
- Gemini grounding
- LLM tool-use/function-calling
- search in voice cleanup
- changing `validate_comment_output`
- parallel `tokio::join!` optimization
