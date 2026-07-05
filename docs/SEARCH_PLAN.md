# План: провайдер-нейтральный поиск фактов для первого комментария

## Архитектура

```
пост канала
  → gemma (Ollama API): сама решает что проверить, ищет первоисточник, формирует запросы
  → mcpr (MCP stdio клиент) → search MCP-сервер (web + GitHub + Reddit)
  → сырые результаты → короткий блок фактов
  → блок фактов в gemini-промпт как «свежий контекст»
  → gemini комментарий (валидатор не меняется)
```

gemma автономно: нет хардкода сущностей, она сама определяет что проверяемо в данном посте и формирует поисковые запросы с фокусом на первоисточник новости. Сменить search-backend = поменять MCP-сервер в конфиге, без правки Rust-кода.

## Как типовой пост проходит через pipeline

**Пост:** «Дефицит памяти продлится до 2028 ... спрос +36%, производство +19% ... 😎Не теряем связь»

1. `should_generate_comment` → true (есть «Не теряем связь»)
2. `clean_post_for_llm` → текст без подписи
3. **gemma extract** (автономно, без хардкода) →
   ```
   NEED_SEARCH: yes
   QUERIES:
   - web: "DRAM memory shortage forecast 2028 demand supply analysis"
   - reddit: "r/hardware DRAM shortage 2026 2027 discussion"
   ```
4. **MCP search** → 2 запроса × 4-5 результатов
5. **Блок фактов в gemini-промпт:**
   ```
   Свежие факты из поиска:
   - DRAM дефицит: TrendForce прогнозирует до Q2 2028 (web)
   - r/hardware: споры о темпах роста цен (reddit)
   ```
6. gemini комментирует, опираясь на пост + факты
7. `validate_comment_output` отбивает если модель начала выдумывать

**Простой пост (некролог, праздник):** gemma возвращает `NEED_SEARCH: no` → MCP-шаг пропускается, комментарий как сейчас.

## Этап 1 — абстракция поиска

**Новые файлы:** `src/features/search/{mod,types,provider}.rs`

- `SearchClaim`, `SearchQuery` (text + source: GitHub/Reddit/Web), `SearchResult` (title/url/snippet/source), `SearchContext` (claims + queries + results + skipped)
- Trait `SearchProvider` с `search()` и `health()` — провайдер-нейтральный, позволяет позже воткнуть прямой API

## Этап 2 — MCP клиент через `mcpr`

**Новые файлы:** `src/mcp/mod.rs`, `src/mcp/search_provider.rs`

- mcpr берёт на себя stdio transport, JSON-RPC 2.0, handshake, tools/list, tools/call
- Один mcpr Client на lifecycle бота (spawn + graceful shutdown)
- `search()` мапит `SearchQuery.source` → имя tool, парсит result → `SearchResult`
- Ошибка/timeout → лог + пустой Vec (не ронять комментарий)
- Рестарт child при падении, healthcheck на startup

**Конфиг:** `SEARCH_ENABLED`, `SEARCH_MCP_COMMAND`, `SEARCH_MCP_ARGS`, `SEARCH_MCP_ENV`, `SEARCH_MCP_TIMEOUT_SEC`, `SEARCH_MCP_TOOLS`

## Этап 3 — gemma extraction (автономная)

**Новые файлы:** `prompts/search_extract.md`, `src/features/search/extract.rs`

Вызов через существующий `generate_text_with_provider(config, Some("ollama"), ...)`. Промпт указывает gemma:
- Прочитать пост, определить есть ли проверяемые утверждения (даты, цифры, прогнозы, цены, релизы)
- Если да → сформулировать 1-3 поисковых запроса для поиска первоисточника новости
- Сама выбрать источники (web для новостей/аналитики, github для кода/benchmark-ов, reddit для обсуждений)
- Если пост не содержит проверяемых фактов (некролог, праздник, опрос) → `NEED_SEARCH: no`

**Без хардкода сущностей.** gemma сама решает что важно в конкретном посте.

Defensive парсинг: мусор → `skipped=true`, комментарий без поиска.

## Этап 4 — интеграция в pipeline

**Изменить:** `pipeline.rs`, `prompt.rs`

- Между `load_relevant_memory_notes` и `generate_text_checked`: `run_search()` → `SearchContext`
- `build_llm_prompt` получает `Option<&SearchContext>`, добавляет блок фактов между RAG и постом
- При `SEARCH_ENABLED=false` — поведение идентично текущему
- Рекомендация: кэш `post_search_cache` с TTL 24-72ч, `extract` параллельно с memory+photo через `tokio::join!`

## Этап 5 — диагностика

- Owner preview: `search: 2 queries, 8 results (web+reddit)` или `search: skipped`
- Логи: `tracing::info!` с claims/queries/results count и latency

## Этап 6 — тесты

- Unit: парсинг gemma extraction (валидный/мусор/`need_search: no`)
- Unit: MCP-result → SearchResult, `build_llm_prompt` с контекстом и без
- Обновить все test `fn config()` новыми полями

## Этап 7 — конфиг и документация

- `config.rs`: новые поля + `validate_runtime_secrets` проверка MCP-команды
- `.env.example`: секция SEARCH_*
- `AGENTS.md`: search pipeline секция
- `docs/TECHNICAL.md`: контур, провайдер-нейтральность
- `docs/LOCAL_WORKFLOW.md`: как локально поднять MCP-сервер

## Этап 8 — deploy (vps-153)

- MCP-сервер на VPS (npm/pnpm или binary)
- `SEARCH_*` env через `EnvironmentFile=.env` в systemd
- Секреты (GITHUB_API_KEY и т.д.) — в `.env`, бот прокидывает только перечисленные в `SEARCH_MCP_ENV`

## Не делаем в этой итерации

- pgvector/embeddings для memory notes
- Изменение `validate_comment_output`
- Search в voice cleanup
- LLM tool-use/function-calling
- Gemini grounding
- Любые breaking changes (SEARCH_ENABLED=false = текущее поведение)

## Риски

| Риск | Митигация |
|------|-----------|
| gemma плохо формирует запросы | Defensive парсинг + skipped=true |
| MCP-сервер падает | mcpr рестарт, healthcheck, поиск optional |
| Latency +1-3с | Timeout, кэш, async parallel |
| Шумные результаты | В промпте «приоритет ниже поста» |
| Rate limits | Кэш, web-only fallback |
