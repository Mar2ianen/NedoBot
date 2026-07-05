# TODO: внедрение MCP-поиска для первого комментария

## Правила итерации

- [ ] Не менять поведение при `SEARCH_ENABLED=false`.
- [ ] Не менять `validate_comment_output`.
- [ ] Не добавлять DB cache/migrations в первой итерации.
- [ ] Не добавлять long-lived MCP client в `AppState`.
- [ ] MCP process запускается lazy per search-run.
- [ ] Все ошибки поиска превращаются в `SearchContext::skipped(...)`, а не ломают комментарий.

## 1. Config

- [x] Добавить `SearchMcpTools` в `src/config.rs`.
- [x] Добавить поля `search_*` в `Config`.
- [x] Добавить env parsing для SEARCH-полей.
- [x] Добавить `validate_search_config`.
- [x] Gated validation: выполнять search validation только если `search_enabled=true`.
- [x] Обновить все test `fn config()` после изменения `Config`.
- [x] Обновить `.env.example` секцией SEARCH.

## 2. Search module skeleton

- [ ] Создать `src/features/search/mod.rs`.
- [ ] Создать `src/features/search/types.rs`.
- [ ] Создать `src/features/search/provider.rs`.
- [ ] Создать `src/features/search/extract.rs`.
- [ ] Создать `src/features/search/mcp.rs`.
- [ ] Создать `src/features/search/service.rs`.
- [ ] Добавить `pub mod search;` в `src/features/mod.rs`.

## 3. Extract prompt and parser

- [ ] Создать `prompts/search_extract.md`.
- [ ] Реализовать JSON response structs.
- [ ] Реализовать strip fenced JSON.
- [ ] Реализовать query sanitize/dedupe/truncate.
- [ ] Реализовать вызов `generate_text_with_provider` с `SEARCH_EXTRACT_PROVIDER/MODEL`.
- [ ] Добавить unit-тесты parser/sanitizer.

## 4. Lazy MCP provider

- [ ] Проверить актуальный API `mcpr`.
- [ ] Если `mcpr` подходит — добавить dependency.
- [ ] Если `mcpr` не подходит — реализовать минимальный stdio JSON-RPC client в `mcp.rs`.
- [ ] Добавить `tokio` features `process`, `time`.
- [ ] Реализовать запуск `SEARCH_MCP_COMMAND` на один search call.
- [ ] Передавать только env vars из `SEARCH_MCP_ENV`.
- [ ] Реализовать source → tool name mapping.
- [ ] Реализовать timeout.
- [ ] Реализовать parsing array output и `{ results: [...] }` output.
- [ ] Не логировать secrets/env values.
- [ ] Добавить unit-тесты parsing/normalization.

## 5. Search service

- [ ] Реализовать `run_search(config, clean_post) -> SearchContext`.
- [ ] Disabled → skipped `disabled`.
- [ ] Extract failed → skipped `extract_failed`.
- [ ] No queries → skipped `no_search_needed`.
- [ ] No results → skipped `no_results`.
- [ ] Dedupe results by URL.
- [ ] Limit results to `MAX_SEARCH_RESULTS`.
- [ ] Добавить tests для skipped states и dedupe.

## 6. First-comment prompt integration

- [ ] Обновить signature `build_llm_prompt(..., search_context: Option<&SearchContext>)`.
- [ ] Добавить render search block.
- [ ] Не включать raw URLs в prompt.
- [ ] Добавить tests: facts included, URLs excluded, skipped block.
- [ ] Обновить все call sites.

## 7. First-comment pipeline integration

- [ ] В `pipeline.rs` вызвать `run_search(config, &clean_post).await` перед `build_llm_prompt`.
- [ ] Передать search context в prompt only if search enabled.
- [ ] Расширить owner preview search summary.
- [ ] Search errors не должны возвращаться из `maybe_comment_post`.

## 8. Documentation

- [ ] Обновить `docs/TECHNICAL.md` секцией Search.
- [ ] Обновить список env vars в `docs/TECHNICAL.md`.
- [ ] Не ссылаться на `docs/LOCAL_WORKFLOW.md`, пока файла нет.

## 9. Validation

- [ ] `cargo fmt`
- [x] `cargo test config`
- [ ] `cargo test search`
- [ ] `cargo test first_comment`
- [ ] `cargo test`
