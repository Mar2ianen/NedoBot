# TG AI Bot Teloxide

Telegram-бот на Rust/teloxide для `НедоNews Chat`.

Текущая MVP-задача: бот помогает живому Telegram-чату не терять контекст. Основные контуры: первый комментарий под постом канала, память/RAG для новостей, статистика чата и расшифровка голосовых через Groq ASR + LLM cleanup.

## Что Уже Работает

- Читает сообщения из `НедоNews Chat`, если privacy mode выключен до добавления бота в чат.
- Сохраняет входящие сообщения в Postgres.
- Распознаёт авто-форварды из канала по `forward_origin.channel.id`.
- Пропускает рекламу/служебные посты без маркера `Не теряем связь`.
- Скачивает самое большое фото поста и отправляет его в модель, если текущий task route profile поддерживает изображения.
- Генерирует комментарий через единый `genai` transport с явным adapter/profile routing для `ollama`, `groq`, `cerebras`, `openrouter`, `openai_compat` и Gemini.
- Отправляет HTML-комментарий reply под постом.
- Отключает link preview.
- Подставляет premium/custom emoji по тематике, включая канал/AMD/Radeon/Ryzen.
- Пишет задачи и результаты генерации в Postgres.
- После комментария асинхронно создаёт атомарную Gemma-карточку полезного поста; рекламу, мемы и повторы помечает `ignored`.
- Ищет релевантную историю через RuBERT Tiny 2 и pgvector с отдельными similarity, temporal coefficient и итоговым rank score.
- Подмешивает последние ответы бота в prompt, чтобы не повторять одинаковые CTA.
- Опционально добавляет свежий web/GitHub/Reddit факт-чек для первого комментария через lazy MCP process, если включён `runtime.search_enabled`.
- Собирает статистику чата с дневной/недельной/месячной отсечкой в 05:00 МСК.
- Показывает пользователей в отчётах человекочитаемо: имя кликабельно, ID спрятан в `tg://user`, рядом статус/админство.
- Сохраняет новые reaction updates, reaction count updates и chat member updates, если Telegram отдаёт их боту.
- Расшифровывает `voice`, `audio` и `video_note`, если включены `runtime.voice_transcription_enabled` и `runtime.voice_auto_transcribe`.
- Для аудиозаписей делает Groq ASR, LLM cleanup, safe Telegram HTML render и audit в `voice_transcription_jobs`.
- Короткие расшифровки отправляет plain text без глав/таймкодов; длинные может отправлять главами с expandable blockquotes или preview + `.txt` файлом.
- Отвечает на `/ask` как агентный помощник: ищет по истории и reply-веткам, разрешает участников, читает безопасные профили/заметки, использует web/GitHub и передаёт фото из reply vision-модели.

## Важный Нюанс Telegram

Если у бота был включён privacy mode, его надо:

1. Отключить в BotFather:

```text
/mybots -> @nedostraj_bot -> Bot Settings -> Group Privacy -> Turn off
```

2. Удалить бота из группы.
3. Добавить бота обратно.

Без re-add Telegram может продолжать отдавать только команды/reply, даже если `getMe` уже показывает `can_read_all_group_messages=true`.

Проверка:

```bash
curl "https://api.telegram.org/bot$TELOXIDE_TOKEN/getMe"
```

Нужно:

```json
"can_read_all_group_messages": true
```

## Конфиг

Локальный `.env` не коммитится и содержит только секреты либо чувствительные URL. Несекретный runtime-конфиг хранится в обязательной секции `[runtime]` существующего файла [`config/llm_profiles.toml.example`](../config/llm_profiles.toml.example). Если секция отсутствует, startup завершается ошибкой — runtime defaults не подставляются. Для production рекомендуется скопировать этот файл в `/etc/tg-ai-bot/llm_profiles.toml` и задать `LLM_PROFILES_PATH` в unit/process environment; если переменная не задана, используется репозиторный `config/llm_profiles.toml.example` только для локального запуска.

В окружении процесса остаются только секреты и чувствительные адреса:

```env
TELOXIDE_TOKEN=
DATABASE_URL=postgres://tg_ai_bot:tg_ai_bot@localhost:5432/tg_ai_bot
CHAT_INVITE_URL=
LLM_PROXY_URL=
GROQ_API_KEY=
CEREBRAS_API_KEY=
GEMINI_API_KEY=
OLLAMA_API_KEY=
OPENAI_COMPAT_API_KEY=
OPENROUTER_API_KEY=
ASK_DATABASE_URL=
GITHUB_PERSONAL_ACCESS_TOKEN=
```

Для systemd production unit путь задаётся явно и абсолютно:

```ini
[Service]
Environment=LLM_PROFILES_PATH=/etc/tg-ai-bot/llm_profiles.toml
```

Не использовать под systemd относительный `config/llm_profiles.toml.example` и не заменять production profile простым копированием example: provider topology и значения `[runtime]` должны быть перенесены из фактического deployment-конфига.

`config/llm_profiles.toml.example` содержит provider/model profiles, task routes и все статические лимиты, флаги, идентификаторы чатов, пути и tool allowlists. API keys, DSN, invite URL и proxy URL туда не переносятся.

### Кандидаты для динамической конфигурации в БД

В БД имеет смысл вынести только policy, которую нужно менять без перезапуска: `comment_blocked_source_domains`, `comment_blocked_terms`, feature flags/moderation thresholds и access policy для `/ask` (`ask_private_user_ids`, список администраторов). Для этого сначала нужны additive migration, typed read-model, явный приоритет `DB > TOML`, version/audit записи и безопасный cache/reload протокол. В этой миграции DB override не включён, поэтому единственным runtime source остаётся `[runtime]` TOML.

Provider credentials, DSN, invite/proxy URLs, transport topology, model routes, timeouts и resource limits в БД переносить не следует: это deployment-контракт и startup validation.

`nedobot.chickenkiller.com` — публичный HTTPS-домен проекта. Он отдаёт только
кэшированные аватарки Telegram по пути `/tg-ai-bot-static/avatars/`; бот строит
их URL из `PUBLIC_BASE_URL`. Production-конфиг общего SNI-фронта лежит в
`deploy/vpn-nginx/nginx.conf`; сертификат Let’s Encrypt обновляется Certbot, а
deploy hook перезагружает контейнерный Nginx после продления.

Для комментариев profile route использует Gemini chain из `config/llm_profiles.toml.example`; fallback-порядок и capability declarations задаются только этим route.

### Строгие LLM profiles

В актуальной profile topology provider дополнительно задаёт genai adapter и egress boundary. Route resolver проверяет capabilities для изображений, native tools, system prompt и output limit; proxy-route без LLM_PROXY_URL отклоняется на startup.

### Единый genai transport и egress

GenAiTransport создаёт два долгоживущих клиента: direct и proxied, если задан LLM_PROXY_URL. Profile provider выбирает egress явно через egress = "direct" или "proxy". Telegram polling, MCP и прочие HTTP-клиенты в этот proxy boundary не входят. Ошибки transport преобразуются в безопасные доменные категории без provider response body.

`LLM_PROFILES_PATH` необязателен только для локального запуска: без него загружается `config/llm_profiles.toml.example`. Для production unit обязан задавать абсолютный `LLM_PROFILES_PATH=/etc/tg-ai-bot/llm_profiles.toml`; на относительный путь под systemd рассчитывать нельзя. Каждая генерация использует явный task route (`first_comment`, `memory`, `voice_cleanup`, `search_extract`, `new_user_audit` или `ask`). Выбранная модель route задаёт driver, base URL, model ID, capabilities, request timeout и `api_key_env`; provider/model overrides через env больше не поддерживаются.

`runtime.render_timezone` задаёт IANA-зону для явного time rendering; текущий deployment использует `Europe/Moscow`. Значение проверяется на старте через teloxide feature `rich-text`, поэтому неизвестная зона останавливает запуск. Общий semantic Rich Text pipeline доступен через canonical `teloxide::utils::rich_text` и имеет три явных frontend-а: HTML (`<tg-time>`, `<tg-emoji>`, `<a href>`), developer Markdown (`@time(...)`, `:alias:`, `[label](alias)`) и LLM Markdown (`14:::00/`, `now+3h/`, `:alias:`, `[label](alias)`). `/ask` использует LLM frontend с одним `RichTextRenderContext`: `chat` всегда разрешается из конфигурации, `message_<id>` строится только для реально наблюдавшихся сообщений, `source_N` — из URL, реально возвращённых web/GitHub search, а custom emoji aliases добавляются только для настроенных ID. Literal URL, включая explicit-scheme raw/bare URL вне code spans и link destinations, в ответе допускается только если он присутствовал во входном вопросе/reply, был возвращён trusted tool evidence или входит в application allowlist; обычный dotted текст без URI scheme не сканируется как URL, а остальные destination отклоняются до delivery. Время захватывается один раз за render call. Progress preview формируется отдельно; compiled payload используется для final delivery и всех внутренних retry окончательной отправки. `Instant` задаёт точный абсолютный момент. `CivilDateTime` задаёт локальное civil time и при DST gap/fold разрешается детерминированной compatible policy, поэтому не является заранее точным instant. Bare clock дополнительно привязывается к локальной дате из одного `captured_now`; если для события нельзя детерминированно выбрать одну fold-инстанцию, вызывающий код обязан передать `Instant`.

Civil date/time и bare clock нормализуются через эту зону с compatible DST disambiguation: пропущенное локальное время сдвигается вперёд, неоднозначное выбирается детерминированно. Для точного автоматического события нужно передавать `Instant`; `CivilDateTime` остаётся локальным временем с deterministic compatible resolution, а bare clock — best-effort представлением.

Целевая топология без Gemini вне комментариев: `/ask` использует Ollama Cloud `minimax-m3`, unified `new_user_audit` — Cerebras `gemma-4-31b`, а Gemini-модели остаются только в цепочке `first_comment`. Unified audit сам обрабатывает аватар и первое сообщение в одном запросе; отдельных avatar/first-message pipelines и jobs больше нет.

На старте каждый включённый route разрешается с его фактическими требованиями к изображению, system prompt и числу output tokens. Для каждого совместимого fallback selection проверяется заданная secret env-переменная; ошибка называет только имя переменной, но не её значение. `structured_output = "prompt_only"` намеренно не передаёт OpenAI-compatible `response_format`: JSON-контракт остаётся в prompt и проверяется typed output validator. Полная topology приведена в `config/llm_profiles.toml.example`.

Если Gemini недоступен напрямую из региона сервера, `LLM_PROXY_URL` может направить только LLM/Gemini-запросы через HTTP/SOCKS proxy, не трогая Telegram polling. На текущем `vps-153` Gemini-трафик идёт через `LLM_PROXY_URL=socks5h://127.0.0.1:2080`, который поднимает systemd-сервис `gemini-proxy-ssh.service` SSH-туннелем до `vps-85`.

Для Gemini 3.x бот использует актуальный `thinkingLevel=low` и не передаёт устаревшие `temperature` и числовой `thinkingBudget`. `runtime.llm_max_tokens` задаёт полный лимит вывода; для JSON-комментария нужен запас, поэтому значение по умолчанию — 180. Для старых Gemini-моделей сохраняется `runtime.gemini_thinking_budget`: бот отправляет `maxOutputTokens = runtime.llm_max_tokens + runtime.gemini_thinking_budget`.

На старте основной сервис и `retry_pending_comments` делают fail-fast проверку секретов для включённых функций:

- Загруженный profile TOML должен быть валидным; секреты проверяются по `api_key_env` всех включённых route selections.
- Если включены `runtime.voice_transcription_enabled=true` и `runtime.voice_auto_transcribe=true`, `runtime.voice_asr_provider=groq` требует `GROQ_API_KEY`.
- Voice cleanup использует profile route `voice_cleanup` и его fallback chain.
- `runtime.new_user_audit_enabled=true` запускает единственный unified worker через route `new_user_audit`. `runtime.new_user_audit_max_tokens` ограничивает его output и по умолчанию равен `900`. После refresh профиля baseline и job сохраняются атомарно; worker сохраняет assessment, materialize-ит итоговый score/signals и upsert-ит review request. Для scoring первого сообщения нужны корректные `runtime.rag_embedding_url`, `runtime.rag_embedding_model` и `runtime.rag_embedding_timeout_sec`.

Это специально ловит ситуацию, когда конфиг переключили на Gemini, но ключ на сервере пустой: бот не стартует с тихим уходом в fallback.

`/ask` использует два независимых deadline: `runtime.ask_action_timeout_sec` ограничивает один native agent turn LLM (с одной retry-попыткой после timeout), а `runtime.ask_total_timeout_sec` ограничивает исследование целиком, включая MCP и внешние tools. Между turn-ами сохраняется полная genai chat history, включая assistant tool calls, call_id-связанные tool responses и thought signatures. Значения `0` запрещены.

MCP и локальные `/ask` tools передаются как `genai::chat::Tool`. Canonical имена с namespace-точкой сохраняются в allowlist, audit и execution policy; на provider wire они получают обратимый alias с `__`, потому что OpenAI-compatible function-name contracts не принимают dotted identifiers. Перед исполнением alias разрешается обратно в canonical имя.

Telegram lifecycle `/ask` полностью использует shared Drafter: каждое progress-событие проходит через synchronous `DraftSink` с latest-wins/coalescing, начальный preview принудительно отправляется через `flush`, а scheduler сам применяет shared limiter, throttle, retry/backoff и native-draft watchdog. В личке во время исследования отправляется настоящий native rich draft; в группах, где Telegram native drafts недоступны, один rich message отправляется и редактируется in place до финального ответа с reply на исходную команду. Успешный ответ и failure-message проходят через `finish`; при подтверждённом отказе worker перед возвратом ошибки best-effort чистит временный preview, а при `Unknown` его не трогает. `abort` остаётся штатным явным путём отмены; limiter общий для всех `/ask`-драфтеров процесса.

Для `/ask` финальная модель ответа сначала проходит LLM time formatter с явно захваченным `now`, затем compiled `RenderedMessage.rich_message` передаётся в Drafter без повторного рендера. Progress preview строится отдельно из статуса текущего agent lifecycle. Готовый Markdown валидируется до статуса `delivery_pending`; только после подтверждённой доставки run становится `completed`. При `NotAttempted` или подтверждённом `Rejected` допускается безопасный fallback, при `Unknown` второе сообщение запрещено. При безопасном fallback Drafter best-effort удаляет временный progress preview; при `Unknown` preview не трогается. `ask_runs` сохраняет исходный Markdown отдельно от compiled Markdown, captured `now`, dialect, timezone, renderer revision и delivery outcome/certainty для аудита и immutable replay. Счётчик `state.ask_delivery_metrics.snapshot()` предоставляет process-local unknown-delivery metric для observability exporter-а.

### Поиск фактов для первого комментария

SEARCH-контур добавляет вспомогательный свежий контекст перед генерацией первого комментария:

```text
clean post -> extract JSON queries -> lazy MCP process -> SearchContext -> build_llm_prompt -> generate_text_checked
```

Поведение gated by config:

- `runtime.search_enabled=false` сохраняет старое поведение: search-блок не добавляется в prompt, а генерация идёт без внешнего поиска.
- Profile route `search_extract` задаёт LLM, который из очищенного поста возвращает JSON с максимум 4 запросами для `web`, `github` или `reddit`.
- `runtime.search_mcp_command` и `runtime.search_mcp_args` запускают основной MCP server лениво на один search-run. Long-lived MCP client в `AppState`, lifecycle restart/shutdown и постоянный child process не используются в первой итерации.
- `runtime.search_mcp_env` — allowlist имён env vars, которые можно передать MCP child process. Значения не логируются.
- `runtime.search_query_timeout_sec` — отдельный deadline одного source query. Таймаут GitHub, Reddit или web не отбрасывает результаты остальных источников.
- `runtime.search_mcp_tool_web`, `runtime.search_mcp_tool_github`, `runtime.search_mcp_tool_reddit` задают имена MCP tools для основного MCP server.
- `runtime.search_mcp_tool_fetch` включает дополнительный fetch top URL после search. Для Exa это `web_fetch_exa`.
- `runtime.search_github_mcp_command` / `runtime.search_github_mcp_args` включают отдельный GitHub MCP server для запросов `source=github`; если они не заданы, GitHub-запросы идут через основной `runtime.search_mcp_tool_github`.
- `runtime.search_github_mcp_env` по умолчанию пропускает только `PATH,HOME,GITHUB_PERSONAL_ACCESS_TOKEN`; значения не логируются.
- `runtime.search_github_mcp_tools` по умолчанию вызывает только read-only `search_issues,search_code`; write tools GitHub MCP не вызываются.
- Для GitHub results бот дополнительно дочитывает top-N URL через read-only `get_issue` / `get_file_contents`: issue/PR body, `README.md`, `CHANGELOG.md`, release docs и другие blob-файлы попадают в snippet как `Fetch: ...`.
- `SEARCH_FETCH_TOP_N` ограничивает число URL для fetch, `SEARCH_FETCH_MAX_CHARS` — объём текста на страницу.
- Ошибка extract превращается в skipped `SearchContext`; ошибка или таймаут отдельного MCP source оставляет успешные результаты других источников доступными для комментария.
- Результаты поиска добавляются в JSON-контекст без raw URL и имеют приоритет ниже текста поста. В промпт помещается до 24 результатов, до 16 000 символов на результат и до 160 000 символов суммарно; URL остаётся только в `SearchContext` для безопасного рендера.
- Каждый search-run сохраняется в `search_runs` для аналитики: статус, skipped reason, latency, queries/results как `jsonb`. Кэша результатов пока нет — запись аналитическая, не влияет на генерацию.
- Chat retrieval работает отдельно: `runtime.chat_retrieval_shadow_enabled` сохраняет гибридные кандидаты и раскрытый контекст только для аудита. `runtime.chat_retrieval_evidence_enabled` по умолчанию выключен; включать его можно лишь после ручной оценки shadow-выборки. Даже при включении в prompt попадают только кандидаты не ниже `runtime.chat_retrieval_evidence_min_score`.

Проверенный вариант без отдельного API key — hosted Exa MCP через `mcp-remote`:

```toml
[runtime]
search_enabled = true
search_mcp_command = "npx"
search_mcp_args = ["-y", "mcp-remote", "https://mcp.exa.ai/mcp"]
search_mcp_env = ["PATH", "HOME"]
search_mcp_timeout_sec = 30
search_query_timeout_sec = 20
search_mcp_tool_web = "web_search_exa"
search_mcp_tool_github = "web_search_exa"
search_mcp_tool_reddit = "web_search_exa"
search_mcp_tool_fetch = "web_fetch_exa"
search_fetch_top_n = 4
search_fetch_max_chars = 16000
```

Для новостей об утилитах можно добавить GitHub MCP поверх Exa, чтобы `source=github` ходил в GitHub issues/code отдельно:

```env
GITHUB_PERSONAL_ACCESS_TOKEN=
```

```toml
[runtime]
search_github_mcp_command = "npx"
search_github_mcp_args = ["-y", "@modelcontextprotocol/server-github"]
search_github_mcp_env = ["PATH", "HOME", "GITHUB_PERSONAL_ACCESS_TOKEN"]
search_github_mcp_tools = ["search_issues", "search_code"]
```

`PATH,HOME` нужны не Exa, а `npx`/`mcp-remote` после `env_clear()`. Значения не логируются.

Voice transcription (`[runtime]` в profile TOML):

```toml
[runtime]
voice_transcription_enabled = false
voice_auto_transcribe = false
voice_max_duration_sec = 600
voice_max_file_mb = 20
voice_short_text_max_chars = 400
voice_language = "ru"
voice_asr_provider = "groq"
voice_asr_model = "whisper-large-v3"
voice_asr_temperature = 0.0
voice_cleanup_temperature = 0.2
voice_cleanup_max_tokens = 1800
voice_render_expandable_chapters = true
voice_send_full_file = true
```

Для изображений в постах первого комментария используется отдельный лимит:

```toml
[runtime]
first_comment_max_image_mb = 10
```

Если Telegram сообщает размер файла выше лимита, бот не скачивает изображение и продолжает генерацию текстового комментария.

Правила voice-конфига:

- `runtime.voice_transcription_enabled=false` полностью выключает voice pipeline, включая `/transcribe`.
- `runtime.voice_auto_transcribe=false` выключает обработку обычных сообщений, но оставляет доступной ручную `/transcribe` reply-команду.
- `runtime.voice_asr_provider=groq` - сейчас единственный поддержанный ASR provider.
- `runtime.voice_asr_model=whisper-large-v3` - дефолт для точной мультиязычной расшифровки в пределах Free Plan лимитов Groq.
- Voice cleanup всегда использует profile route `voice_cleanup` и его fallback chain.
- `runtime.voice_short_text_max_chars=400` значит короткая расшифровка после cleanup отправляется как простой текст без глав и времени.
- `runtime.voice_max_file_mb=20` выбран под cloud Bot API `getFile`; для больших файлов нужен local Bot API server.
- Если обычный HTML не влезает в безопасный лимит Telegram, бот отправляет Rich Message с закрытым блоком полного текста. `runtime.voice_send_full_file=true` оставляет `preview + voice-transcript.txt` только как fallback при ошибке Rich API или превышении rich-лимита.

## Локальный Запуск

Поднять изолированную локальную PostgreSQL development-базу:

```bash
./scripts/dev_db.sh start
DATABASE_URL=postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_dev cargo run --bin migrate
```

Контейнер `tg-ai-bot-postgres-dev` слушает только `127.0.0.1:5433` и использует отдельный volume; он не пересекается с production PostgreSQL. Полный сценарий reset и policy данных описаны в [`DEVELOPMENT.md`](DEVELOPMENT.md).

Запустить бота:

```bash
cargo run
```

Проверка:

```bash
cargo check
```

## Тесты

Быстрый набор без контейнеров:

```bash
cargo test --all-targets
```

Полный DB-aware suite:

```bash
./scripts/test.sh
```

Runner запускает локальный Podman PostgreSQL, пересоздаёт `tg_ai_bot_test`, применяет migrations и выполняет PostgreSQL integration tests. В CI используется такой же образ `pgvector/pgvector:0.8.2-pg16-bookworm`: migrations и ignored integration tests запускаются отдельными шагами.

## VPS Деплой

Текущий production release на `vps-153` фиксируется immutable tag [`deploy-2026-08-02-semantic-rich-text`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-08-02-semantic-rich-text) после успешной сборки и post-deploy smoke. В него входят merged `main`, shared Drafter, semantic Rich Text frontends, explicit time rendering, delivery certainty, durable voice delivery lifecycle и production LLM profile. Предыдущие releases: [`deploy-2026-08-02-drafter-time-rendering`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-08-02-drafter-time-rendering), [`deploy-2026-07-31-teloxide-fork`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-07-31-teloxide-fork), [`deploy-2026-07-31-v0.12.0`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-07-31-v0.12.0), [`deploy-2026-07-31-voice-flow`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-07-31-voice-flow) и [`deploy-2026-07-30-unified-audit`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-07-30-unified-audit). Полный порядок dry-run, выкладки, проверки, rollback и фиксации tag описан в [`docs/DEPLOYMENT.md`](DEPLOYMENT.md).


- код: `/opt/tg-ai-bot-teloxide`
- Postgres: Podman container `tg-ai-bot-postgres`
- systemd:
  - `container-tg-ai-bot-postgres.service`
  - `tg-ai-bot-teloxide.service`
  - `nedobot-rag-embedding.service`
  - `nedonews-mcp.service`

PostgreSQL запускается из образа `pgvector/pgvector:0.8.2-pg16-bookworm` на том же persistent volume. RuBERT Tiny 2 обслуживается локальным CPU-only Text Embeddings Inference на `127.0.0.1:8788`; наружу этот порт не публикуется.

Полезные команды:

```bash
ssh vps-153 'systemctl status tg-ai-bot-teloxide --no-pager'
ssh vps-153 'systemctl status nedonews-mcp --no-pager'
ssh vps-153 'journalctl -u tg-ai-bot-teloxide -f'
ssh vps-153 'podman ps'
```

## Публичный Read-only MCP

`https://nedobot.chickenkiller.com/mcp/nedonews/v2` — намеренно публичный MCP Streamable HTTP endpoint с данными только `НедоNews Chat`. Версия в URL отделяет RMCP-контракт от удалённого legacy JSON-RPC API: внешний клиент обязан выполнить `tools/list`, а не переиспользовать старые input/output schemas. Endpoint не даёт ни SQL, ни shell, ни доступ к `public.*`: отдельная PostgreSQL-роль `nedobot_mcp_ro` читает лишь явно перечисленные views схемы `mcp_public`.

- Миграция `20260717180000_mcp_public_views.sql` задаёт scope и explicit-колонки. Foreign/private chat scope и raw Telegram API JSON не выдаются как общий доступ; personal-channel поля, явно включённые в `mcp_public`, входят в фактический контракт ниже. Полный reviewed inventory опубликованных view и полей находится в [`MCP_PUBLIC_DATA.md`](MCP_PUBLIC_DATA.md).
- `config/mcp_db_manifest.toml` — проверяемый allowlist views, колонок и их типов, а [`MCP_PUBLIC_DATA.md`](MCP_PUBLIC_DATA.md) — его human-readable snapshot. При старте MCP сверяет manifest с БД и отказывается стартовать при schema drift.
- Внешнему клиенту доступны только структурированные `db.*` и read-only domain tools; значения передаются bind-параметрами, лимит одной страницы — 200, effective column list — 40. Generic page собирается до logical rows budget 480 KiB, затем возвращает корректные `has_more`/`next_cursor`; запас учитывает дублирование RMCP text и structured content в wire response. Широкие views требуют явно передать `columns`. Одно text-поле может содержать до 8192 символов, domain message tools возвращают preview до 4096 символов; при превышении text, JSONB и array поля сообщают `_truncated_fields`, а preview заканчивается `…`. Aggregate `min`/`max` возвращает полное значение либо контролируемую ошибку budget. Соединений с БД — два, `statement_timeout` — 5 секунд.
- `db.search_text` остаётся manifest-инструментом для одной разрешённой текстовой колонки. Domain-инструменты `chat.search_messages`, `chat.search_messages_batch` и `chat.count_messages` используют общий typed search service и доступны одновременно локальному `/ask` RMCP child process и публичному Streamable HTTP router; локальный allowlist ограничивает только `/ask`, а не меняет read-model.
- `chat.search_messages` по умолчанию использует `match_mode: "hybrid"`: русский/simple full-text плюс короткое fuzzy-сопоставление через `pg_trgm`. Допустимы `full_text`, `any_terms`, `literal` и `whole_word`; для альтернативных формулировок используй `any_terms`, для точного термина — `whole_word` или `literal`. Результат имеет форму `{messages, total_count, has_more, next_offset, scan_limit_reached}`; для следующей страницы передай возвращённый `next_offset` как `offset` (допустимый диапазон 0–10000). Если достигнут потолок сканирования, `has_more=true`, `next_offset=null`, `scan_limit_reached=true`; поэтому top-k не следует принимать за полный набор.
- `chat.search_messages_batch` выполняет до шести независимых запросов и возвращает метаданные `total_count`/`has_more`/`next_offset`/`scan_limit_reached` для каждого запроса. `chat.count_messages` выполняет отдельный aggregate count с теми же predicates, без сортировки и relevance ranking; он возвращает число matching-сообщений, а не число событий или вхождений слова внутри одного сообщения. Поэтому он предназначен для вопросов «сколько сообщений»/«в скольких сообщениях», а не для occurrence count, уникальных авторов или событий по голому «сколько раз»/«как часто».
- Даты принимаются как RFC 3339 или `YYYY-MM-DD`; дата без времени для `date_from` означает начало UTC-дня, а для `date_to` — его конец. По умолчанию поиск исключает сообщения без пользователя, ботов и автоматические пересылки; `include_forwards=true` включает пересылки явно, в том числе строки без сохранённого автора, для вопросов о канале или forwarded content.
- Добавление fuzzy search не меняет `mcp_public` views, scope или sanitization: это только новый read-only query path поверх уже опубликованной проекции. Миграция `20260803090000_chat_search_quality.sql` добавляет `pg_trgm` и индекс для этого пути.
- JSON рекурсивно очищается от ключей наподобие `token`, `secret`, `authorization`, `database_url` и `invite_link`. В логах сохраняются только tool, table/columns/operators, количество строк и latency — без текстов сообщений и значений фильтров.

### Public MCP data exposure contract

Это фактический и сознательно принятый контракт экспозиции, а не обещание privacy-minimized projection. HTTP adapter по умолчанию слушает только `127.0.0.1` и не использует application authentication; если deployment настраивает внешний reverse proxy, именно он расширяет доступность endpoint-а и отвечает за внешний контроль доступа.

`mcp_public` остаётся curated read model с reviewed scope и allowlist-ом, но обычные внутренние данные публичного chat read-model не скрываются только потому, что они внутренние. В зависимости от view и domain tool внешнему клиенту доступны, среди прочего:

- `profile_photo_file_unique_id` и другие `file_unique_id` профиля или медиа;
- сведения и последний текст personal channel;
- raw voice transcripts, ASR segments и final render;
- LLM prompts, responses и final output;
- вопросы и ответы `/ask`, audit-поля и tool arguments;
- anti-spam risk scores, reasons и labels;
- admin event payload;
- chat notes и user notes;
- тексты сообщений и job errors.

Sanitization удаляет только распознанные secret-like JSON keys и не является общим фильтром приватности. SQL, запись, shell и доступ к foreign/private chat scope по-прежнему не выдаются. Изменение этого набора — отдельный reviewed contract change; в текущем PR views, manifest и sanitization намеренно не меняются.

Публикация новой таблицы или колонки — отдельный reviewed change: правка projection view, затем генерация и review manifest. Автоматически новые поля не раскрываются:

```bash
cargo run --release --bin generate_mcp_db_manifest -- config/mcp_db_manifest.toml
git diff -- config/mcp_db_manifest.toml
```

Подготовка роли выполняется на сервере администратором (пароль не хранится в репозитории):

```bash
podman exec -i tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot \
  -v mcp_password='GENERATE_A_LONG_RANDOM_PASSWORD' \
  -f - < deploy/nedonews-mcp/bootstrap-role.sql
```

Unit `deploy/nedonews-mcp/nedonews-mcp.service` читает только `/etc/nedobot/nedonews-mcp.env`; туда не передаются Telegram или LLM secrets и ему не нужен writable checkout. Nginx проксирует исключительно `/mcp/nedonews/v2` на `127.0.0.1:8787`, принимает body не больше 64 KiB и ждёт upstream 70 секунд — дольше 60-секундного application deadline.

Ручной redeploy из локальной папки выполняется только после dry-run и проверки production profile; не использовать старую сокращённую команду без exclusions:

```bash
rsync -azn --delete --exclude target --exclude .git --exclude '.env*' --exclude static/ --exclude backups/ --exclude '*.dump' --exclude docs/LOCAL_WORKFLOW.md ./ vps-153:/opt/tg-ai-bot-teloxide/
rsync -az --delete --exclude target --exclude .git --exclude '.env*' --exclude static/ --exclude backups/ --exclude '*.dump' --exclude docs/LOCAL_WORKFLOW.md ./ vps-153:/opt/tg-ai-bot-teloxide/
ssh vps-153 'chmod 755 /opt/tg-ai-bot-teloxide && runuser -u tg-ai-bot -- test -x /opt/tg-ai-bot-teloxide'
rsync -az config/llm_profiles.toml.production.example vps-153:/etc/tg-ai-bot/llm_profiles.toml
ssh vps-153 'cd /opt/tg-ai-bot-teloxide && /root/.cargo/bin/cargo build --release'
ssh vps-153 'systemctl restart tg-ai-bot-teloxide && systemctl is-active tg-ai-bot-teloxide'
ssh vps-153 'systemctl restart nedonews-mcp && systemctl is-active nedonews-mcp'
```

## База

Главные таблицы:

- `telegram_messages` - входящие сообщения и raw Telegram JSON.
- `post_comment_jobs` - дедупликация и статус комментария под постом.
- `llm_generations` - prompt, модель, ответ LLM и финальный HTML.
- `post_history_entries` - атомарная история новых постов: строгий Gemma-summary, сущности, использованный ракурс, реально использованный внешний факт и RuBERT embedding. Исходные посты не склеиваются.
- `voice_transcription_jobs` - job/status/raw ASR/segments/cleaned transcript/final HTML/file id для расшифровки голосовых.
- `telegram_user_profiles` - последние виденные username/name/is_bot/is_premium, а также best-effort детали из `getChat(user_id)`, `getUserProfilePhotos` и `getUserPersonalChatMessages`: bio, avatar file ids, emoji status/accent, personal channel summary/raw JSON и ошибки API.
- `telegram_chat_users` - явная расширяемая карточка пользователя в конкретном чате: первое/последнее сообщение, счётчики сообщений/реплаев/ссылок/медиа, статус в чате, админство, join/leave/invite-link поля.
- `telegram_chat_member_snapshots` - последний известный статус пользователя в чате.
- `telegram_chat_member_events` - входы, выходы и изменения статусов, если Telegram прислал update.
- `telegram_message_reactions` - персональные изменения реакций.
- `telegram_message_reaction_counts` - последние известные счётчики реакций по сообщению.
- `bot_settings`, `telegram_users`, `telegram_chats`, `admin_events` - задел под админку.

Спам-разметка:

- `telegram_messages.spam_type` - нормализованный тип спама для конкретного сообщения.
- `telegram_chat_users.spam_type` - основной тип спамера.
- `telegram_chat_users.spam_types` - JSON-счётчик типов по пользователю.
- `telegram_chat_users.spam_profile_labels` - признаки профиля: generic female avatar/persona и другие сильные контекстные маркеры именно этого чата. Рандомный username сам по себе не считать сильной метрикой: в чате это частая норма.
- текущие seed-типы: `llm_generic_comment`, `promo_dm_bait`, `adult_personal_channel_promo`.
- `llm_generic_comment` - безобидно выглядящий LLM-коммент по теме поста, часто с одинаковым восторженным тоном.
- `promo_dm_bait` - промо через “могу отправить/поделиться/пишите в личку”, тематика может быть разная, но механика одна.
- `adult_personal_channel_promo` - личный/personal channel пользователя ведёт на adult-промо, инвайт-ссылки или схожий funnel.
- Для первого текстового сообщения сохраняются LLM-маркеры кампании, RuBERT-вектор и сходство с вручную подтверждённым спамом. Эти сигналы лишь повышают review-риск; автоматической пометки спамером нет.

Для каждого нового пользователя бот сохраняет один audit-запрос в
`spam_review_requests`, включая low и medium risk. Карточка для ревью с тегом
`@Chechulinm` отправляется только когда актуальный `risk_score >= 70` (`high`):
порог проверяется и перед Telegram API call, и DB constraint'ом при claim/delivery.
Пользователь с меньшим score не может получить карточку даже при ошибочном caller-е.
Поздние сигналы аватара или первого сообщения могут сделать уже сохранённый audit
доставляемым. Кнопки «Верно: спамер» и «Неверно: не спамер» доступны только
`runtime.owner_telegram_id`; первое решение атомарно закрывает запрос и убирает
клавиатуру. Технические labels риска в карточке переводятся в понятные причины. Кнопки доступны только владельцу, заданному через `runtime.owner_telegram_id`.

Посмотреть последние сообщения:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select chat_id, message_id, source_channel_id, source_message_id, is_automatic_forward, left(coalesce(text, ''), 200) as text, created_at from telegram_messages order by id desc limit 20;\""
```

Посмотреть задачи комментариев:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select * from post_comment_jobs order by id desc limit 20;\""
```

Посмотреть атомарную историю:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select source_message_id, status, summary, entities, used_angle, external_fact, skip_reason, created_at from post_history_entries order by id desc limit 20;\""
```

Посмотреть voice jobs:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select id, chat_id, message_id, media_kind, duration_sec, file_size, status, asr_provider, asr_model, render_mode, left(coalesce(error, ''), 120) as error, created_at, updated_at from voice_transcription_jobs order by id desc limit 20;\""
```

## Команды Бота

```text
/ping
/db
/emojiids
/format_test <текст поста>
/memory
/ask <вопрос>
/status day|week|month [-r|-p]
/stats_day [-r|-p]
/stats_week [-r|-p]
/stats_month [-r|-p]
/topmsg [-r|-p]
/topreact [-r|-p]
/userstats <id|username> [-r|-p]
/userstatus <id|username> [-r|-p]
```

В группах лучше писать с username:

```text
/ping@nedostraj_bot
```

`/stats_day`, `/stats_week` и `/stats_month` показывают имена пользователей как скрытые ссылки на Telegram-профиль, без видимого ID. Рядом выводятся короткие бейджи: `админ`, `в чате`, `не в чате`, `бот` или `статус неизвестен`.

`/userstats` принимает числовой Telegram ID, уже виденный ботом username или reply на сообщение пользователя. Без аргумента команда показывает отправителя. `UserStatsArgs` один раз нормализует command arguments: render-флаги `-r`/`--rich` и `-p`/`--plain` можно поставить до или после target, они не считаются частью username, а команда только с флагом сохраняет reply/sender fallback. Нормализованный target используется и для refresh профиля, и для построения отчёта. В общих отчётах ID намеренно не печатается; для точного SQL-разбора он остаётся в таблицах `telegram_messages`, `telegram_user_profiles` и `telegram_chat_users`.


## Prompt

### Chat retrieval (shadow rollout)

Gemma строит единый `ResearchPlan`: главный subject/audience, secondary context, chat semantic/lexical queries и запросы к внешним источникам. Shadow retrieval объединяет RuBERT vector, PostgreSQL full-text и безопасные literal-regex совпадения за 30 дней с geometric freshness. Кандидаты и ограниченные ветки сохраняются в `chat_research_runs`, но не меняют комментарий без ручной проверки.

`CHAT_AUTHOR:id` и `CHAT_MESSAGE:id:label` разрешены только для ID из подтверждённого retrieval-контекста. Имя автора берётся только из `first_name`; при username ссылка ведёт на профиль, иначе на сообщение. Без evidence обязателен обычный `CHAT_LINK`.

Основной prompt лежит в [prompts/first_comment.md](../prompts/first_comment.md).
Короткий факт-чек/RAG для защиты от устаревших утверждений лежит в [prompts/tech_rag.md](../prompts/tech_rag.md).
Cleanup prompt для расшифровки голосовых лежит в [prompts/voice_cleanup.md](../prompts/voice_cleanup.md).

Модель первого комментария возвращает structured JSON: `{"comment":"...","used_search_result_id":null}`. В `comment` обязателен ровно один `{CHAT_LINK}` или вариант с разрешённым текстом ссылки вроде `{CHAT_LINK:чате}` / `{CHAT_LINK:комментах}`. Gemini получает JSON Schema через API, Ollama fallback — ту же schema через `format`; для остальных совместимых провайдеров сохраняется строгий JSON-контракт в prompt.

Если поиск вернул безопасный результат с публичным HTTP(S) URL, модель обязана выбрать один отдельный угол, которого нет в новости: связанный релиз, ограничение, последствие, сравнение, цену, changelog или реакцию сообщества. Поиск нельзя использовать только для подтверждения или пересказа факта из поста. `used_search_result_id: null` допускается только при пустом или небезопасном поиске. One-based ID сохраняется в `llm_generations`, а `{SOURCE_LINK:N:подпись}` становится обязательным. Подпись должна быть частью фразы («как пишет VideoCardz»), а не отдельным «детали» или «источник». `COMMENT_BLOCKED_SOURCE_DOMAINS` исключает указанные домены и поддомены до fetch, из prompt и при финальном рендере; `COMMENT_BLOCKED_TERMS` так же исключает результаты и комментарии с заданными фрагментами текста. Search response сохраняется до best-effort fetch: неуспешный fetch не удаляет title/snippet уже найденного источника. Output validator отклоняет факт без источника, raw URL, битые/лишние плейсхолдеры, неподходящий ID, текст длиннее 180 видимых символов и generic CTA. Код сам рендерит ссылки в HTML, а предпросмотр ссылок отключён для обычных и rich text send-путей.
RAG не предназначен для пересказа новости: пост канала важнее, а карточки нужны только чтобы не писать ложные вещи вроде `Switch 2 еще не вышла`.

Автоматическая история работает поверх RuBERT Tiny 2 и pgvector:

- после успешной отправки комментария создаётся отдельная job с исходным постом, комментарием бота и только реально выбранным результатом поиска;
- Gemma получает строгую JSON Schema через provider API и возвращает `summary`, `entities`, `used_angle`, `external_fact`, `skip_reason`;
- `summary: null` разрешён для рекламы, мемов, служебных публикаций, повторов и постов без устойчивого полезного факта; запись становится `ignored` и не участвует в retrieval;
- полезная запись получает 312-мерный embedding `cointegrated/rubert-tiny2` и становится `ready`;
- перед внешним поиском бот строит embedding нового поста и выбирает до шести карточек по cosine similarity;
- рейтинг считается как `similarity * temporal_coefficient`, где коэффициент свежести плавно снижается от `1.0` к `0.70`, а период полураспада настраивается через `runtime.rag_temporal_half_life_days`;
- Gemma-поисковик видит `already_known` и `already_used_angles`, поэтому ищет развитие истории, последствия, альтернативы, changelog или свежую реакцию, а при отсутствии нового направления может вернуть `need_search=false`;
- модель комментария получает одновременно найденную историю и свежие результаты внешнего поиска;
- старые объединённые заметки удаляются миграцией и не переносятся в новую историю.

Антиповтор CTA:

- перед генерацией бот достаёт последние 12 ответов из `llm_generations`;
- prompt просит не повторять их начало, глаголы CTA и общий рисунок фразы;
- это снижает повторы вроде `залетайте`, `заходите`, `сравним`, `обсудим`.

## Расшифровка Голосовых

Pipeline вызывается в `handle_message` до first-comment pipeline:

```rust
match maybe_transcribe_voice(&bot, &msg, &state).await {
    Ok(true) => return Ok(()),
    Ok(false) => {}
    Err(err) => tracing::error!(%err, "failed to process voice transcription"),
}
```

Порядок обработки:

1. Проверить `runtime.voice_transcription_enabled` и `runtime.voice_auto_transcribe` для автоматического режима; `/transcribe` требует только `runtime.voice_transcription_enabled`.
2. Отфильтровать чужие чаты, ботов, команды и automatic forwards.
3. Определить `VoiceMedia` из `voice`, `audio` или `video_note`.
4. Сохранить исходное Telegram message в `telegram_messages`.
5. Создать или возобновить `voice_transcription_jobs`; повтор того же `(chat_id, message_id)` не создаёт дубликат и не мешает reclaim просроченного lease.
6. Проверить duration/file size до скачивания.
7. Скачать файл через Telegram `getFile` во временный файл.
8. Для `video_note` задать multipart MIME `video/mp4` и отправить исходный MP4 в Groq `/audio/transcriptions`.
9. Сразу после preflight отправить reply `Расшифровка…`; для обычного результата заменить его через `editMessageText`, а для Rich/file варианта обновить его статусом и отправить полный payload отдельным сообщением.
10. Сохранить raw ASR text, segments и raw JSON.
11. Запустить LLM cleanup по `prompts/voice_cleanup.md`.
12. Нормализовать clean result: короткий текст остаётся short, пустые/битые главы отбрасываются.
13. Собрать Telegram HTML через `telegram::html`.
14. Перевести job из `cleaning` в `delivering` перед первым постоянным Telegram side effect.
15. Отправить reply: одно сообщение или preview + `voice-transcript.txt`.
16. После подтверждённой доставки сохранить cleaned text, chapters JSON, final HTML и file id и перевести job в `sent`.
17. Каждый job claim-ится через `FOR UPDATE SKIP LOCKED`, получает lease и CAS-переходы по `attempts`; pre-send/transient failure переводит его в bounded `retry_wait`, исчерпание retry — в `failed`.
18. Неоднозначный network/timeout результат доставки, а также ошибка DB-finalization после успешной отправки, переводит job в `delivery_unknown` без автоматической повторной доставки. Просроченный lease в `delivering` также восстанавливается в `delivery_unknown`, а не подбирается как обычный processing job.

ASR request:

```text
POST https://api.groq.com/openai/v1/audio/transcriptions
model = runtime.voice_asr_model
response_format = verbose_json
language = runtime.voice_language
temperature = runtime.voice_asr_temperature
timestamp_granularities[] = segment
```

Cleanup request:

- сначала используется profile route `voice_cleanup` и его fallback chain;
- если все cleanup selections падают, используется raw ASR transcript;
- если JSON от модели не парсится или cleanup меняет объём/числа сверх безопасных границ, используется raw ASR transcript.

Rendering policy:

- `clean.text.chars().count() <= runtime.voice_short_text_max_chars` -> только исправленный текст;
- `mode=chapters` + непустые chapters -> заголовок `Расшифровка голосового` и главы;
- тело главы идёт в `<blockquote expandable>`, если `runtime.voice_render_expandable_chapters=true` и обычное сообщение влезает;
- если HTML длиннее `SAFE_TEXT_LIMIT=3900`, бот отправляет Rich Message с закрытым `<details>`; rich-формат поддерживает до 32 768 символов;
- если Rich API отклоняет сообщение или rich-лимит превышен, `runtime.voice_send_full_file=true` включает fallback `preview + voice-transcript.txt`.

Текущий важный нюанс: `TranscriptChapter.start_sec` уже хранится, но `render.rs` пока не выводит timestamp рядом с заголовком главы. Это ближайший фикс в [REFACTOR_NEXT.md](REFACTOR_NEXT.md).

`video_note` Telegram не сопровождает MIME-типом, поэтому pipeline задаёт `video/mp4` сам. Groq принимает MP4 напрямую: отдельный `ffmpeg` и постоянное хранение кружков не нужны. Временный файл удаляется сразу после завершения ASR-запроса.

Cleanup prompt находится в `prompts/voice_cleanup.md`. Он должен чистить ASR, а не пересказывать голосовое: сохранять спорные формулировки автора, не менять числа/версии/названия моделей и учитывать локальный контекст канала `НедоNews`. В частности, `Gemma 4 31B` / `gemma4:31b` — валидная модель проекта, её нельзя заменять на `Gemma 2`, `Gemini` или `27B`.

## New User Audit

`src/features/new_user_analysis.rs` собирает профильные и поведенческие метрики новых/низкоактивных пользователей. Live flow запускает аудит после refresh профиля автора сообщения; `message_count >= 5` считается old-active baseline: snapshot сохраняется, но риск-сигналы не начисляются.

`runtime.new_user_audit_enabled=false` по умолчанию. При включении после profile refresh создаётся только unified job: один LLM assessment содержит profile, avatar и first-message sections, после чего bounded materialization атомарно обновляет score/signals и review request. Startup validation проверяет profile route, output limit и embedding-конфиг; параллельных источников риска и отдельных legacy jobs нет.

Ключевая таблица: `telegram_new_user_profile_audits`. В ней сохраняются классы риска, labels/reasons, возраст в чате, reply/comment context, текстовая повторяемость, профиль/персональный канал, наличие/метрики фото. `profile_photo_reuse_count` сейчас метрика only и не добавляет risk score.

## Метрики И Отладка

Отсечки периодов:

- день: сегодня с `05:00` по Москве;
- неделя: понедельник `05:00` по Москве;
- месяц: первое число месяца `05:00` по Москве.

Сводка по сообщениям:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select count(*) as messages, count(*) filter (where is_automatic_forward) as auto_forwards, count(*) filter (where source_channel_id is not null) as from_channel, min(created_at) as first_seen, max(created_at) as last_seen from telegram_messages;\""
```

Скорость отправки комментариев:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select source_message_id, round(extract(epoch from updated_at - created_at)::numeric, 2) as send_pipeline_sec, status, bot_comment_message_id from post_comment_jobs order by source_message_id desc limit 20;\""
```

### Reconciliation ambiguous first-comment delivery

`delivery_unknown` означает, что Telegram transport не подтвердил результат fenced send. Такая задача **никогда не переотправляется автоматически**. Для оператора есть отдельный CLI; он не применяет миграции:

```bash
# Только чтение ambiguous задач / одной задачи.
cargo run --bin reconcile_comment_delivery -- list --limit 20
cargo run --bin reconcile_comment_delivery -- inspect --job-id 123

# Подтверждённый факт доставки или отсутствия доставки: только DB-переход + audit.
cargo run --bin reconcile_comment_delivery -- mark-delivered --job-id 123 --bot-comment-message-id 456 --actor alice --reason "reply verified in discussion"
cargo run --bin reconcile_comment_delivery -- mark-failed --job-id 123 --actor alice --reason "no bot reply after manual inspection"

# Риск дубля принят оператором явно. Только после точного claim создаются Config/Bot и запускается настоящий pipeline.
cargo run --bin reconcile_comment_delivery -- retry --job-id 123 --actor alice --reason "verified no reply" --acknowledge-duplicate-risk
```

Все operator actions пишутся в `post_comment_job_operator_audit` с bounded `actor` (1–128 символов), `reason` (1–1000), исходным и итоговым status. `delivery_unknown` не claim-ят ни normal worker, ни `retry_pending_comments`; они могут reclaim-ить только просроченную pre-send `processing` задачу с `operator_retry_only`. Pre-send/confirmed rejection при такой попытке terminally fail без `retry_wait` и очищают `operator_retry_only`; подтверждённый `sent` также очищает флаг. Network ambiguity снова становится `delivery_unknown`, сохраняет `operator_retry_only` и требует нового решения оператора. После каждого operator retry outcome (`sent`, `failed`, `delivery_unknown`) добавляется append-only audit, включая транзакционный переход expired `sending -> delivery_unknown` в normal claim path: из-за уже применённого CHECK action записывается существующее разрешённое значение `retry`, а outcome указан в reason.

Реакция людей за 30 минут после комментария:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"with metrics as (select j.source_message_id, count(m.*) filter (where m.created_at <= j.created_at + interval '5 minutes' and coalesce(m.text,'') !~ '^/') as msg_5m, count(m.*) filter (where m.created_at <= j.created_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/') as msg_30m, count(distinct m.user_id) filter (where m.created_at <= j.created_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/') as users_30m from post_comment_jobs j left join telegram_messages m on m.chat_id = j.discussion_chat_id and m.created_at > j.created_at and m.created_at <= j.created_at + interval '30 minutes' and m.message_id <> j.bot_comment_message_id and m.user_id is distinct from 8907803505 and m.source_channel_id is null group by j.source_message_id, j.created_at, j.bot_comment_message_id) select round(avg(msg_5m)::numeric, 2) as avg_msg_5m, round(avg(msg_30m)::numeric, 2) as avg_msg_30m, round(avg(users_30m)::numeric, 2) as avg_users_30m from metrics;\""
```

Реакции на комментарии бота:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select j.source_message_id, j.bot_comment_message_id, coalesce(rc.total_count, 0) as reactions, rc.reactions from post_comment_jobs j left join telegram_message_reaction_counts rc on rc.chat_id = j.discussion_chat_id and rc.message_id = j.bot_comment_message_id order by j.created_at desc limit 20;\""
```

Формат отчётов:

- `Топ пользователей` исключает служебного авто-форвард пользователя Telegram `777000`, ботов и сами посты канала.
- Пользователь выводится как кликабельное имя с HTML-ссылкой `tg://user?id=...`; видимый ID не печатается, чтобы отчёт читался нормально в чате.
- Статус берётся из `telegram_chat_member_snapshots`: Telegram `administrator/owner` показываются как админские статусы, `member` как `в чате`, `left/banned` как отсутствие в чате.
- `/userstats` дополнительно показывает первое и последнее увиденное ботом сообщение пользователя по `telegram_chat_users`; без аргумента выбирается отправитель команды, а если команду отправить reply на сообщение, пользователь выбирается из reply.
- `Завлечение после коммента` считает среднее число некомандных сообщений после комментария бота за 5 минут, 30 минут и 24 часа, плюс среднее число уникальных людей за 30 минут. Отчётный период выбирает cohort комментариев; их 5м/30м/24ч окна намеренно могут продолжаться за его правую границу.
- `Комменты бота` сортируются по обсуждению за 30 минут, прямым реплаям и реакциям. Текст очищается от HTML/AI-маркеров и обрезается до короткого превью.
- Period-данные собирает `features/stats/service.rs` в `ChatStatsReportData`; `render_html.rs` и `render_rich.rs` получают одну typed-модель и не выполняют SQL. SQL и repository DTO находятся в `features/stats/repo.rs`.
- Аватар в `/userstats` обогащается только для Rich-отчёта; plain HTML-вариант не вызывает Telegram API и локальный avatar cache ради неиспользуемого изображения.

Что важно помнить по данным:

- Старые сообщения частично добиты миграцией из `raw_json`, но старые реакции Telegram Bot API не отдаёт.
- Reaction events и reaction count updates будут нулевыми, пока Telegram не начнёт присылать такие апдейты боту.
- Join/leave и точные member-status события зависят от того, какие `chat_member` updates Telegram реально отдаёт боту. На старте бот дополнительно делает best-effort `getChatMember` по последним виденным пользователям.
- Автоматическая конверсия по отдельной invite-ссылке пока не считается; входы через конкретную ссылку можно будет выделить, когда Telegram начнёт отдавать invite link в member events.

## Импорт Telegram Export

Для старой истории чата используется отдельная CLI-команда, не polling-бот:

```bash
cargo run --bin import_telegram_export -- "/path/to/ChatExport/result.json" --dry-run
cargo run --release --bin import_telegram_export -- "/path/to/ChatExport/result.json"
```

Импорт читает `result.json` из Telegram/AyuGram Desktop export, вычисляет Bot API chat id из export id (`1932061163` -> `-1001932061163`) и пишет данные в текущие таблицы:

- `telegram_messages`;
- `telegram_user_profiles`;
- `telegram_chat_users`.

Дедупликация:

- сообщения пишутся через `telegram_messages unique(chat_id, message_id)`;
- профили пишутся через `telegram_user_profiles primary key (telegram_user_id)`;
- пользовательская статистика пересобирается из `telegram_messages` в `telegram_chat_users`, поэтому повторный импорт не увеличивает счётчики;
- live Bot API `raw_json` не затирается экспортным JSON при конфликте, импорт только дополняет отсутствующие поля и флаги;
- forwarded channel messages и automatic channel posts различаются: `sender_chat_id` заполняется только для реального `from_id/actor_id=channel...`, а `source_channel_id` может хранить как auto-forward source, так и forwarded source.

Перед импортом на VPS лучше сделать backup:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres pg_dump -U tg_ai_bot -d tg_ai_bot -Fc -f /tmp/tg_ai_bot_before_export_import.dump"
ssh vps-153 "podman cp tg-ai-bot-postgres:/tmp/tg_ai_bot_before_export_import.dump /opt/tg-ai-bot-teloxide/tg_ai_bot_before_export_import.dump"
```

## Наблюдаемость lifecycle jobs

`job_lifecycle_report` — локальный read-only отчёт для operational state очередей:

```bash
cargo run --bin job_lifecycle_report
```

Команде требуется только `DATABASE_URL`; она не создаёт `Config`, не проверяет LLM/Telegram secrets и не запускает миграции. Все запросы определены в typed read-model `features::jobs::observability` и выполняются внутри `SET TRANSACTION READ ONLY`.

Отчёт охватывает `first-comments`, `embeddings`, `post-history` и `reviews`: число jobs и суммарные attempts по статусу, `oldest_ready_age` для старейшей due initial/retry job, безопасные группы `error_kind` с attempts и terminal failures, а также суммарный `lease_reclaim_count`. Expired processing leases не входят в ready-age. Для reviews predicate совпадает с ready-частью production claim: `status = pending`, `risk_score >= 70`, notification `pending/retry_wait` и due time. Неизвестный persisted error kind не выводится: он агрегируется как `other`. Для embeddings отдельно показан текущий счётчик rows с `embedding_batch_cardinality`.

`lease_reclaim_count` сохраняется в доменной таблице и увеличивается только когда worker действительно забирает просроченную `processing` lease. Обычный claim из `pending`/`retry` и повторная попытка после явной failure-finalization его не увеличивают. Для reviews используется аналогичное поле `notification_lease_reclaim_count` её delivery lifecycle.

### Preflight optional index для просроченных spam-review leases

Существующий partial index `spam_review_requests_notification_ready_idx` обслуживает due `pending`/`retry_wait` reviews. Отдельного индекса для reclaim ветки `notification_status = 'processing' AND notification_lease_expires_at <= now()` сейчас нет намеренно: добавлять migration следует только после production evidence, а не заранее.

На production сначала снять размер очереди и число реально reclaimable leases (в read-only сессии):

```sql
select
    count(*) as total_reviews,
    count(*) filter (
        where status = 'pending'
          and risk_score >= 70
          and notification_status = 'processing'
          and notification_lease_expires_at <= now()
    ) as expired_processing_ready,
    count(*) filter (
        where status = 'pending'
          and risk_score >= 70
          and notification_status in ('pending', 'retry_wait')
          and notification_next_attempt_at <= now()
    ) as due_initial_or_retry
from spam_review_requests;
```

Затем на репрезентативной production нагрузке проверить reclaim predicate отдельным планом:

```sql
explain (analyze, buffers)
select id
from spam_review_requests
where status = 'pending'
  and risk_score >= 70
  and notification_status = 'processing'
  and notification_lease_expires_at <= now()
order by notification_lease_expires_at, id
limit 1;
```

И обязательно снять план полного candidate query из production claim: `OR` между due и expired ветками вместе с общим `ORDER BY` может выбрать другой план, чем isolated reclaim predicate.

```sql
explain (analyze, buffers)
select id
from spam_review_requests
where status = 'pending'
  and risk_score >= 70
  and (
    (notification_status in ('pending', 'retry_wait') and notification_next_attempt_at <= now())
    or (notification_status = 'processing' and notification_lease_expires_at <= now())
  )
order by notification_next_attempt_at, id
limit 1;
```

Migration на второй partial index допустима только если одновременно наблюдаются ненулевая/растущая очередь expired `processing` rows, план выполняет дорогое scan/sort без подходящего index и claim latency становится измеримой operational проблемой. При нулевой или эпизодической очереди, либо если текущий plan остаётся дешёвым, migration не создавать. Любое решение добавить индекс требует сохранить эти результаты (queue counts, `EXPLAIN ANALYZE` и latency) в review migration.

## Custom Emoji

Список считанных premium/custom emoji:

- [docs/custom_emoji_stickers.tsv](custom_emoji_stickers.tsv)
- [docs/custom_emoji_sheet.png](custom_emoji_sheet.png)

Текущие ID:

```toml
[runtime]
comment_custom_emoji_id = "5445092965875729965"
tech_custom_emoji_id = ""
amd_custom_emoji_id = "5442995600201106682"
radeon_custom_emoji_id = "5442853853395436819"
ryzen_custom_emoji_id = "5444875271163364561"
```

## Ограничения MVP

- Новая RAG-история начинается с момента миграции без backfill старых объединённых заметок.
- RuBERT Tiny 2 работает на CPU и оценивает смысловую близость; окончательное решение о полезности карточки и направлении поиска остаётся за Gemma.
- Реакции считаются только с момента включения reaction updates; старые реакции Telegram Bot API задним числом не отдаёт.
- Статусы пользователей известны по последнему `chat_member` update или по будущим снимкам; если Telegram не присылал событие, статус будет `unknown`.
- Если LLM provider вернёт ошибку/subscription limit, задача может остаться без комментария до ручного вмешательства.
- Voice ASR сейчас только через Groq; local Whisper/Ollama audio не подключены.
- Cleanup provider/model для voice пока не сохраняются в отдельные DB-поля, хотя поля в таблице уже есть.
- Join-конверсия по отдельной invite-ссылке пока не считается автоматически.
- Админки пока нет; статические настройки меняются в `[runtime]` profile TOML и требуют рестарта сервиса. DB-backed dynamic policy — отдельный следующий этап.
