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

- [x] Создать `src/features/search/mod.rs`.
- [x] Создать `src/features/search/types.rs`.
- [x] Создать `src/features/search/provider.rs`.
- [x] Создать `src/features/search/extract.rs`.
- [x] Создать `src/features/search/mcp.rs`.
- [x] Создать `src/features/search/service.rs`.
- [x] Добавить `pub mod search;` в `src/features/mod.rs`.

## 3. Extract prompt and parser

- [x] Создать `prompts/search_extract.md`.
- [x] Реализовать JSON response structs.
- [x] Реализовать strip fenced JSON.
- [x] Реализовать query sanitize/dedupe/truncate.
- [x] Реализовать вызов `generate_text_with_provider` с `SEARCH_EXTRACT_PROVIDER/MODEL`.
- [x] Добавить unit-тесты parser/sanitizer.

## 4. Lazy MCP provider

- [x] Проверить актуальный API `mcpr`.
- [ ] Если `mcpr` подходит — добавить dependency.
- [x] Если `mcpr` не подходит — реализовать минимальный stdio JSON-RPC client в `mcp.rs`.
- [x] Добавить `tokio` features `process`, `time`.
- [x] Реализовать запуск `SEARCH_MCP_COMMAND` на один search call.
- [x] Передавать только env vars из `SEARCH_MCP_ENV`.
- [x] Реализовать source → tool name mapping.
- [x] Реализовать timeout.
- [x] Реализовать parsing array output и `{ results: [...] }` output.
- [x] Не логировать secrets/env values.
- [x] Добавить unit-тесты parsing/normalization.

## 5. Search service

- [x] Реализовать `run_search(config, clean_post) -> SearchContext`.
- [x] Disabled → skipped `disabled`.
- [x] Extract failed → skipped `extract_failed`.
- [x] No queries → skipped `no_search_needed`.
- [x] No results → skipped `no_results`.
- [x] Dedupe results by URL.
- [x] Limit results to `MAX_SEARCH_RESULTS`.
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
