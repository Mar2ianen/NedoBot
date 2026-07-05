# Prompts for search implementation subagents

Эти промпты предназначены для быстрых субагентов. Каждый промпт задаёт конкретный write scope. Не давать двум субагентам один и тот же write scope одновременно.

## Prompt 1 — Config fields and validation

```text
Ты работаешь в Rust-проекте tg-ai-bot-teloxide.

Задача: реализовать только config-часть SEARCH для MCP-поиска. Не трогай search modules, first_comment pipeline, prompts и docs кроме явно указанных файлов.

Write scope:
- src/config.rs
- .env.example
- тестовые Config fixtures только в тех файлах, где компилятор потребует новые поля Config

Точные требования:
1. В src/config.rs добавь #[derive(Clone)] pub struct SearchMcpTools { pub web: String, pub github: String, pub reddit: String }.
2. В Config добавь поля:
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
3. В Config::from_env добавь defaults:
   SEARCH_ENABLED=false
   SEARCH_EXTRACT_PROVIDER=ollama as Some(String)
   SEARCH_EXTRACT_MODEL=gemma4:31b as Some(String)
   SEARCH_EXTRACT_TEMPERATURE=0.1
   SEARCH_EXTRACT_MAX_TOKENS=700
   SEARCH_MCP_COMMAND empty -> None
   SEARCH_MCP_ARGS empty -> Vec::new(), otherwise split_whitespace
   SEARCH_MCP_ENV empty -> Vec::new(), otherwise split comma, trim, drop empty
   SEARCH_MCP_TIMEOUT_SEC=8
   SEARCH_MCP_TOOL_WEB=web_search
   SEARCH_MCP_TOOL_GITHUB=github_search
   SEARCH_MCP_TOOL_REDDIT=reddit_search
4. Добавь private helper env_list_csv(name: &str) -> Vec<String> и env_args(name: &str) -> Vec<String>.
5. Добавь validate_search_config(errors: &mut Vec<String>, config: &Config).
6. В validate_runtime_secrets вызывай validate_search_config и validate_llm_provider_secret/model для SEARCH_EXTRACT_PROVIDER только внутри if self.search_enabled.
7. validate_search_config должен добавить error если search_mcp_command is None или search_mcp_timeout_sec == 0.
8. Обнови все test Config fixtures новыми полями с defaults из пункта 3.
9. В .env.example добавь SEARCH section с переменными из пункта 3. Не добавляй секреты.
10. Запусти cargo fmt --check и cargo test config. В финале напиши команды и результат.
```

## Prompt 2 — Search types, prompt, extract parser

```text
Ты работаешь в Rust-проекте tg-ai-bot-teloxide.

Задача: создать модуль features/search с types, provider trait и extract parser/caller. Не трогай MCP implementation, first_comment pipeline, Config и docs.

Write scope:
- src/features/mod.rs
- src/features/search/mod.rs
- src/features/search/types.rs
- src/features/search/provider.rs
- src/features/search/extract.rs
- prompts/search_extract.md

Точные требования:
1. В src/features/mod.rs добавь pub mod search;.
2. Создай src/features/search/mod.rs с pub mod extract; pub mod provider; pub mod types;.
3. В types.rs добавь:
   - enum SearchSource { Web, Github, Reddit } с serde rename_all = "snake_case".
   - struct SearchQuery { pub source: SearchSource, pub text: String }
   - struct SearchResult { pub source: SearchSource, pub title: String, pub url: String, pub snippet: String }
   - struct SearchContext { pub queries: Vec<SearchQuery>, pub results: Vec<SearchResult>, pub skipped_reason: Option<String>, pub latency_ms: u128 }
   - impl SearchContext { pub fn skipped(reason: impl Into<String>, latency_ms: u128) -> Self; pub fn is_skipped(&self) -> bool; }
   - constants MAX_SEARCH_QUERIES=3, MAX_QUERY_CHARS=180, MAX_SEARCH_RESULTS=8, MAX_RESULT_TITLE_CHARS=140, MAX_RESULT_SNIPPET_CHARS=220.
4. В provider.rs добавь async_trait SearchProvider с async fn search(&self, query: &SearchQuery) -> anyhow::Result<Vec<SearchResult>>.
5. prompts/search_extract.md заполни JSON-only prompt из docs/SEARCH_PLAN.md section "Extract prompt".
6. В extract.rs реализуй:
   pub async fn extract_search_queries(config: &Config, clean_post: &str) -> anyhow::Result<Vec<SearchQuery>>
   Вызов generate_text_with_provider(config, config.search_extract_provider.as_deref(), config.search_extract_model.as_deref(), &prompt, None, config.search_extract_temperature, config.search_extract_max_tokens).
7. В extract.rs реализуй private parse_extract_response(value: &str) -> anyhow::Result<Vec<SearchQuery>>.
8. Parser должен принимать raw JSON и fenced ```json JSON ```.
9. JSON shape: { "need_search": bool, "queries": [{ "source": "web|github|reddit", "text": "..." }] }.
10. Если need_search=false вернуть empty Vec.
11. sanitize: trim, drop empty, truncate to MAX_QUERY_CHARS by chars, dedupe by lowercase text + source, take MAX_SEARCH_QUERIES.
12. Добавь unit tests в extract.rs:
   - parses_valid_json
   - parses_fenced_json
   - no_search_returns_empty_queries
   - drops_duplicate_queries
   - truncates_long_query
13. Запусти cargo fmt --check и cargo test search::extract. В финале напиши команды и результат.
```

## Prompt 3 — Lazy MCP provider

```text
Ты работаешь в Rust-проекте tg-ai-bot-teloxide.

Задача: реализовать lazy MCP search provider. Не трогай Config, first_comment pipeline, prompt.rs и docs.

Write scope:
- Cargo.toml
- src/features/search/mcp.rs
- src/features/search/mod.rs

Точные требования:
1. В Cargo.toml добавь tokio features process и time к существующему tokio dependency. Не удаляй существующие features.
2. Не добавляй mcpr dependency в этой задаче.
3. Реализуй minimal JSON-RPC stdio MCP client через tokio::process::Command в src/features/search/mcp.rs.
4. В search/mod.rs добавь pub mod mcp;.
5. Добавь pub struct McpSearchProvider { config: Config } и impl McpSearchProvider { pub fn new(config: Config) -> Self }.
6. Реализуй SearchProvider for McpSearchProvider.
7. For each search call spawn SEARCH_MCP_COMMAND with SEARCH_MCP_ARGS.
8. Pass only env vars listed in config.search_mcp_env from parent env to child. Use env_clear then set selected env vars if present.
9. Use stdin/stdout piped.
10. JSON-RPC flow:
   a. send initialize request with id 1, method "initialize", params {"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"tg-ai-bot-teloxide","version":"0.1.0"}}
   b. read one JSON response line
   c. send initialized notification method "notifications/initialized"
   d. send tools/call request id 2 with params {"name": tool_name, "arguments": {"query": query.text, "limit": 5}}
   e. read one JSON response line
   f. kill child after parsing result
11. Tool mapping:
   Web -> config.search_mcp_tools.web
   Github -> config.search_mcp_tools.github
   Reddit -> config.search_mcp_tools.reddit
12. Wrap whole operation in timeout Duration::from_secs(config.search_mcp_timeout_sec).
13. On any error, log warn without env values and return Ok(Vec::new()).
14. Parse tool output from MCP response result.content. Accept text content containing JSON. Accept JSON array or object {"results":[...]}.
15. Each result object fields: title, url, snippet. Missing fields become empty string. Drop results with empty title and empty snippet.
16. Truncate title to MAX_RESULT_TITLE_CHARS and snippet to MAX_RESULT_SNIPPET_CHARS by chars.
17. Add tests for pure parser functions only:
   - parses_array_results
   - parses_object_results
   - drops_empty_results
   - truncates_result_fields
18. Expose no network/process integration tests.
19. Запусти cargo fmt --check и cargo test search::mcp. В финале напиши команды и результат.
```

## Prompt 4 — Search service

```text
Ты работаешь в Rust-проекте tg-ai-bot-teloxide.

Задача: реализовать search service orchestration. Не трогай Config, MCP internals, first_comment pipeline, prompt.rs и docs.

Write scope:
- src/features/search/service.rs
- src/features/search/mod.rs

Точные требования:
1. В search/mod.rs добавь pub mod service;.
2. В service.rs реализуй pub async fn run_search(config: &Config, clean_post: &str) -> SearchContext.
3. Если !config.search_enabled вернуть SearchContext::skipped("disabled", latency_ms).
4. Вызвать extract_search_queries(config, clean_post).await.
5. Если extract returns Err, log warn and return skipped("extract_failed", latency_ms).
6. Если queries empty вернуть skipped("no_search_needed", latency_ms).
7. Создать McpSearchProvider::new(config.clone()).
8. Sequentially call provider.search(query).await for each query.
9. If provider returns Err, log warn and continue. Do not return Err.
10. Collect all results.
11. Dedupe results by exact URL string; for empty URL dedupe by lowercase title+snippet.
12. Take MAX_SEARCH_RESULTS.
13. If final results empty return SearchContext { queries, results: Vec::new(), skipped_reason: Some("no_results".to_string()), latency_ms }.
14. Else return SearchContext { queries, results, skipped_reason: None, latency_ms }.
15. Add unit test for dedupe helper only:
   - dedupes_by_url
   - dedupes_empty_url_by_text
   - keeps_different_urls
16. Запусти cargo fmt --check и cargo test search::service. В финале напиши команды и результат.
```

## Prompt 5 — First-comment prompt integration

```text
Ты работаешь в Rust-проекте tg-ai-bot-teloxide.

Задача: обновить build_llm_prompt для SearchContext. Не трогай pipeline.rs, Config, MCP, service и docs.

Write scope:
- src/features/first_comment/prompt.rs
- тесты в этом же файле

Точные требования:
1. Импортируй SearchContext и SearchSource.
2. Измени signature build_llm_prompt на:
   pub fn build_llm_prompt(post_text: &str, chat_member_count: Option<u32>, memory_notes: &[MemoryNote], recent_comments: &[String], search_context: Option<&SearchContext>) -> String
3. Добавь render_search_context(search_context: Option<&SearchContext>) -> String.
4. Если search_context None или skipped или results empty, вернуть "Свежий поиск: нет дополнительного контекста.".
5. Если results есть, вернуть block:
   Свежий поиск, использовать осторожно:
   - Это вспомогательный контекст, он ниже поста по приоритету.
   - Не цитируй URL и не добавляй ссылки.
   - Если поиск противоречит посту, не утверждай спорное как факт.
   - Если результаты нерелевантны, игнорируй их.

   Результаты:
   - [web] title — snippet
6. Source labels exactly: web, github, reddit.
7. Do not include result.url in prompt.
8. Insert search block between tech_rag and memory block.
9. Update existing tests compile errors by passing None.
10. Add tests:
   - search_context_is_rendered_without_urls
   - skipped_search_context_is_explicit
11. Запусти cargo fmt --check и cargo test first_comment::prompt. В финале напиши команды и результат.
```

## Prompt 6 — First-comment pipeline integration

```text
Ты работаешь в Rust-проекте tg-ai-bot-teloxide.

Задача: подключить run_search в first-comment pipeline и owner preview. Не трогай Config, MCP, extract, service implementation, prompt rendering internals и docs.

Write scope:
- src/features/first_comment/pipeline.rs
- при необходимости только call-site tests caused by build_llm_prompt signature

Точные требования:
1. Import run_search and SearchContext.
2. After recent_comments are loaded and before build_llm_prompt call, add:
   let search_context = run_search(config, &clean_post).await;
3. Call build_llm_prompt with fifth arg:
   config.search_enabled.then_some(&search_context)
4. Do not propagate search errors; run_search already returns SearchContext.
5. Change send_owner_preview signature to include search_context: &SearchContext.
6. Owner preview must append line:
   <code>search=2 queries, 6 results, 1420ms</code>
   if skipped_reason None.
7. If skipped_reason Some(reason), append line:
   <code>search=skipped(reason), 1420ms</code>
8. Update caller accordingly.
9. Запусти cargo fmt --check и cargo test first_comment. В финале напиши команды и результат.
```

## Prompt 7 — Technical documentation update

```text
Ты работаешь в Rust-проекте tg-ai-bot-teloxide.

Задача: обновить техническую документацию после внедрения SEARCH. Не трогай Rust-код.

Write scope:
- docs/TECHNICAL.md

Точные требования:
1. В env/config section добавь SEARCH variables exactly:
   SEARCH_ENABLED
   SEARCH_EXTRACT_PROVIDER
   SEARCH_EXTRACT_MODEL
   SEARCH_EXTRACT_TEMPERATURE
   SEARCH_EXTRACT_MAX_TOKENS
   SEARCH_MCP_COMMAND
   SEARCH_MCP_ARGS
   SEARCH_MCP_ENV
   SEARCH_MCP_TIMEOUT_SEC
   SEARCH_MCP_TOOL_WEB
   SEARCH_MCP_TOOL_GITHUB
   SEARCH_MCP_TOOL_REDDIT
2. Добавь секцию "Поиск фактов для первого комментария".
3. Опиши pipeline exactly:
   clean post -> extract JSON queries -> lazy MCP process -> SearchContext -> build_llm_prompt -> generate_text_checked.
4. Напиши, что SEARCH_ENABLED=false сохраняет старое поведение.
5. Напиши, что MCP process запускается per search-run, без long-lived lifecycle.
6. Напиши, что DB cache/migrations не входят в первую итерацию.
7. Не ссылаться на docs/LOCAL_WORKFLOW.md.
8. В финале не запускать cargo test, потому что код не менялся.
```
