# TG AI Bot Teloxide

Telegram-бот на Rust/teloxide для `НедоNews Chat`.

Текущая MVP-задача: бот помогает живому Telegram-чату не терять контекст. Основные контуры: первый комментарий под постом канала, память/RAG для новостей, статистика чата и расшифровка голосовых через Groq ASR + LLM cleanup.

## Что Уже Работает

- Читает сообщения из `НедоNews Chat`, если privacy mode выключен до добавления бота в чат.
- Сохраняет входящие сообщения в Postgres.
- Сохраняет признак любой Telegram-пересылки (`is_forwarded`) и её источник (`forwarded_from`), отдельно от авто-форварда из канала.
- Пропускает рекламу/служебные посты без маркера `Не теряем связь`.
- Скачивает самое большое фото поста и отправляет его в модель, если текущий task route profile поддерживает изображения.
- Генерирует комментарий через единый `genai` transport с явным adapter/profile routing для `ollama`, `groq`, `cerebras`, `openrouter`, `openai_compat` и Gemini.
- Отправляет HTML-комментарий reply под постом.
- Отключает link preview.
- Подставляет premium/custom emoji по тематике, включая канал/AMD/Radeon/Ryzen.
- Пишет задачи и результаты генерации в Postgres.
- После комментария асинхронно создаёт атомарную Gemma-карточку полезного поста; рекламу, мемы и повторы помечает `ignored`.
- Ищет релевантную историю через EmbeddingGemma 2 Q4/512d и pgvector с отдельными similarity, temporal coefficient и итоговым rank score.
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

Не использовать под systemd относительный `config/llm_profiles.toml.example` и не заменять production profile простым копированием example: provider topology и значения `[runtime]` должны быть перенесены из фактического deployment-конфига. При изменении route policy (например, `fallback_on_validation_failure`) нужно синхронно обновить deployment-копию, проверить её до запуска и перезапустить сервис; правка example-файла сама по себе production не меняет.

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

`/ask` использует Groq `qwen/qwen3.8-27b`, затем динамический OpenRouter `openrouter/free`, Ollama Cloud `minimax-m3` и Ollama Cloud `gemma4:31b`. Для каждого запроса output budget ограничивается profile cap выбранного fallback; модель с меньшим cap не выпадает из route. Groq обслуживает vision и reasoning, free router подбирает модель с нужными image/tool capabilities, а модели Ollama с заявленными capabilities остаются fallback-ами. Платные OpenRouter-модели и Gemini в `/ask` не участвуют. Voice cleanup использует отдельную цепочку Groq → OpenRouter Qwen → Ollama. Unified `new_user_audit` — Cerebras `gemma-4-31b`, а Gemini-модели остаются в цепочке `first_comment`. Unified audit сам обрабатывает аватар и первое сообщение в одном запросе; отдельных avatar/first-message pipelines и jobs больше нет.

На старте каждый включённый route разрешается с его фактическими требованиями к изображению, system prompt и числу output tokens. Для каждого совместимого fallback selection проверяется заданная secret env-переменная; ошибка называет только имя переменной, но не её значение. `structured_output = "prompt_only"` намеренно не передаёт OpenAI-compatible `response_format`: JSON-контракт остаётся в prompt и проверяется typed output validator. При отказе output validator LLM service пишет в journal только route, fallback index, provider, model, номер попытки, размер ответа и безопасный `validation_reason`; полный prompt и ответ модели не логируются. Для `first_comment` причины типизированы (`missing_chat_link`, `raw_link`, `generic_cta`, `invalid_json`, `chat_evidence`, `source_link`, `blocked_term` и другие), а тот же код сохраняется в `llm_generations.attempts` при успешном fallback. Полная topology приведена в `config/llm_profiles.toml.example`.

`LLM_PROXY_URL` остаётся опциональной настройкой для явно проксируемых LLM routes. На текущем `vps-153` все активные routes и Gemini ASR используют прямой egress; Telegram polling, MCP и прочие HTTP-клиенты в этот proxy boundary не входят.

Для Gemini 3.x бот использует актуальный `thinkingLevel=low` и не передаёт устаревшие `temperature` и числовой `thinkingBudget`. `runtime.llm_max_tokens` задаёт полный лимит вывода; для JSON-комментария нужен запас, поэтому значение по умолчанию — 180. Для старых Gemini-моделей сохраняется `runtime.gemini_thinking_budget`: бот отправляет `maxOutputTokens = runtime.llm_max_tokens + runtime.gemini_thinking_budget`.

На старте основной сервис и `retry_pending_comments` делают fail-fast проверку секретов для включённых функций:

- Загруженный profile TOML должен быть валидным; секреты проверяются по `api_key_env` всех включённых route selections.
- Если включён voice pipeline, `runtime.voice_asr_provider=gemini` требует `GEMINI_API_KEY`; в двойном режиме второй Groq-текст требует `GROQ_API_KEY`.
- Voice cleanup использует profile route `voice_cleanup` и его fallback chain.
- `runtime.new_user_audit_enabled=true` запускает единственный unified worker через route `new_user_audit`. `runtime.new_user_audit_max_tokens` ограничивает его output и по умолчанию равен `900`. После refresh профиля baseline и job сохраняются атомарно; worker сохраняет assessment, materialize-ит итоговый score/signals и upsert-ит review request. Для scoring первого сообщения нужны корректные `runtime.rag_embedding_url`, `runtime.rag_embedding_model` и `runtime.rag_embedding_timeout_sec`. `runtime.avatar_embeddings_enabled` отдельно включает bounded image-векторы только после явного spam-label; обычные аудит-аватары не сохраняются, при снятии label dataset rows удаляются, raw image bytes не хранятся.

Это специально ловит ситуацию, когда конфиг переключили на Gemini, но ключ на сервере пустой: бот не стартует с тихим уходом в fallback.

`/ask` использует два независимых deadline: `runtime.ask_action_timeout_sec` ограничивает один native agent turn LLM (с одной retry-попыткой после timeout), а `runtime.ask_total_timeout_sec` ограничивает исследование целиком, включая MCP и внешние tools. Между turn-ами сохраняется полная genai chat history, включая assistant tool calls, call_id-связанные tool responses и thought signatures. Значения `0` запрещены.

В tracked profiles бюджет исследования расширен: `ask_max_steps=64`
ограничивает число фактических вызовов tools (включая предварительную
подгрузку reply), а модель получает до 68 turns: 64 основных, 3 коррекционных
и 1 для финального ответа. `ask_total_timeout_sec=1800` — общий потолок;
`ask_action_timeout_sec=180` — одна LLM-попытка, с transport timeout
из model capabilities (для Groq ask — 180 секунд, для OpenRouter Free — 120 секунд);
`ask_db_mcp_timeout_sec=30` — один MCP request.
`ask_llm_max_tokens=16384` задаёт верхний бюджет генерации на один LLM-вызов, включая reasoning.
Для `/ask` каждый выбранный fallback получает динамический параметр
`min(ask_llm_max_tokens, model.capabilities.max_output_tokens)`: Qwen получает до 16k,
OpenRouter Free — до 4k, Ollama Minimax — до 8k. Нижний профильный предел больше не
исключает fallback из route. Для Qwen включён `thinking="level_high"` (effort `high`),
`ask_max_concurrency=4` — число одновременно исполняемых `/ask` в инстансе.
Существующий deployment-profile получает эти значения явно после бэкапа
и restart; увеличение шаблона само по себе production-конфиг не меняет.
Профили memory и first_comment сохраняют собственные task budgets.

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
- `runtime.youtube_subtitles_enabled` добавляет к найденным YouTube URL локальное чтение ручных и auto-субтитров через `runtime.youtube_subtitles_command` (обычно `yt-dlp`). Обогащение выполняется после внешнего MCP search/fetch и доступно одинаково первому комментарию и `/ask`; новый MCP transport для этого не создаётся.
- `runtime.youtube_subtitles_languages` задаёт приоритетные селекторы языков yt-dlp (`ru`, `ru.*`, `en`, `en.*`), `runtime.youtube_subtitles_max_videos` ограничивает число роликов на один source query, а `runtime.youtube_subtitles_max_chars` — объём добавляемого текста. Таймаут subprocess задаётся `runtime.youtube_subtitles_timeout_sec`.
- При включённой функции отсутствие команды/бинарника или некорректные limits — startup error; при отсутствии субтитров, timeout или ошибке конкретного ролика исходный search result сохраняется без transcript.
- Внешний RMCP дополнительно предоставляет `youtube.get_subtitles`: он принимает только public URL конкретного YouTube-видео и возвращает найденный язык и очищенный текст субтитров. Настройки отдельного RMCP-сервиса задаются через `MCP_YOUTUBE_SUBTITLES_*`; по умолчанию инструмент выключен, а включение требует установленного `yt-dlp`.
- RMCP вызывает `yt-dlp` без shell, с `--skip-download` и `--no-playlist`, ограничивает один вызов timeout/размером текста и удаляет временные VTT-файлы после чтения. Плейлисты, credentials в URL и произвольные домены отклоняются до запуска subprocess.
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
voice_asr_provider = "gemini"
voice_asr_model = "gemini-3.5-transcribe"
voice_asr_shadow_enabled = true
voice_asr_shadow_model = "whisper-large-v3-turbo"
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
- `runtime.voice_asr_provider=gemini` и `runtime.voice_asr_model=gemini-3.5-transcribe` задают основной ASR; по результатам сравнения на реальных голосовых Gemini лучше разбирает длинную техническую речь.
- При `runtime.voice_asr_shadow_enabled=true` второй текст строится Groq-моделью из `runtime.voice_asr_shadow_model=whisper-large-v3-turbo`. Если основной ASR недоступен, pipeline использует успешный второй результат.
- Gemini принимает только аудио; для `video_note` MP4 pipeline использует Groq как совместимый fallback.
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

Текущий production release на `vps-153` зафиксирован immutable annotated tag [`deploy-2026-10-08-ask-python-files`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-08-ask-python-files) на merge commit `e1a6ab47b88f3eccfa2100c8857a8cbd544ce0f3`. Код прошёл review через [PR #45](https://github.com/Mar2ianen/NedoBot/pull/45); артефакт из [release workflow](https://github.com/Mar2ianen/NedoBot/actions/runs/37807666497) установлен **2026-10-08 16:26 UTC / 19:26 МСК** в оба community-инстанса и public MCP.

В профиле НедоNews включён ограниченный `sandbox.python` для анализа одного UTF-8 текстового документа из ответа на сообщение (до 2 MiB). Исполнение идёт в одноразовом rootless Podman-контейнере без сети, host mounts и секретов, с ограничением ресурсов; постоянный Jupyter kernel и сохранение результатов отключены. Используется локально закреплённый image digest `sha256:2a890751d3ac217ba36aab6235e15fa29d0f0e28f72ab040afac0d8180a0fdbd` и отдельный пользователь `nedobot-sandbox`. Профиль ПВО feature не включает. Startup preflight успешно выполнил `pass`; rootless CSV и timeout smoke проверены до релиза.

После рестарта все три unit-а active, `NRestarts=0`, ошибок application journal нет; хеши обоих bot binaries совпали с artifact (`5eef9f12104a6c0be1653effe42a039ece06fb3ef21d80c4c55eb9ef746a44cd`), MCP — `a9e4a1c16769431c5569b885dc201a13f45bc8426e981bd9db1d17beb9e4261c`. До включения sandbox сделан закрытый backup `/etc/tg-ai-bot/llm_profiles.toml` в `/opt/tg-ai-bot-releases/deploy-2026-10-08-ask-python-files-before-20261008T162503Z/`.

Предыдущий production release был зафиксирован immutable annotated tag [`deploy-2026-10-06-bot-changelog-details`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-06-bot-changelog-details) на точном source commit `6949a151fc8e9ffd67419a8bb477ebd2b2077fc2`. Код прошёл review через [PR #38](https://github.com/Mar2ianen/NedoBot/pull/38); артефакт из [release workflow](https://github.com/Mar2ianen/NedoBot/actions/runs/37492109948) установлен **2026-10-06 16:09 UTC / 19:09 МСК** в оба community-инстанса и public MCP.

`/ask` подмешивает `docs/BOT_CHANGELOG.md` к system prompt только для вопросов о возможностях и обновлениях самого бота; вопросы о новых сообщениях чата продолжают использовать поиск по истории. Журнал включает релизы 5–6 октября, включая сохранённые spam/not-spam метки в аудите повторяющихся имён и окно доставки review-карточек. В production audit и shared spam labels включены; CAS и review delivery выключены. Runtime profiles и секреты не менялись, новых миграций в этом релизе нет.

`cargo test --all-targets` локально завершился с 433 успешными тестами и 6 штатно ignored; форматирование и Clippy с `-D warnings` прошли. GitHub CI прошёл PostgreSQL migrations/integration, RMCP и moderation checks. После restart все три сервиса active, running executable hashes совпали с артефактом, application error journal пуст; в каждой БД 92 успешные миграции, failed migrations нет. MCP probe вернул ожидаемый `403` локально и `405` публично на unauthenticated GET. Telegram smoke-команды не отправлялись.

SHA256 bot executable обоих инстансов: `d3d910bc5a356fae9771fba0eabd28d20003d901f901bd6fac0ff868d0d636fe`; MCP: `26968db01032cf3f46508e2c3ece07906f0a0cbc2f232d02bf7199762a6140df`. Проверенный stage и deployment result сохранены в `/opt/tg-ai-bot-releases/deploy-2026-10-06-bot-changelog-details-6949a15/`; до выкладки custom dumps обеих БД, consistent SQLite backup, binaries, static/model files, `.env` и profiles сохранены в приватном `/opt/tg-ai-bot-releases/deploy-2026-10-06-bot-changelog-details-before-20261006T155329Z/`. Релизное состояние также записано в `.release.json` обоих инстансов.

### Предыдущая первая выкладка журнала — 6 октября, 18:36 МСК

Tag [`deploy-2026-10-06-bot-changelog`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-06-bot-changelog) на source `6c11dc761872ae6ee7b650fb59fac2e2f7072f46` был выпущен через [PR #36](https://github.com/Mar2ianen/NedoBot/pull/36) и заменён текущим release после дополнения фактов по вчерашним коммитам. Первый artifact и deployment result сохранены в `/opt/tg-ai-bot-releases/deploy-2026-10-06-bot-changelog-6c11dc7/`, predeploy backup — в `/opt/tg-ai-bot-releases/deploy-2026-10-06-bot-changelog-before-20261006T152904Z/`.

### Предыдущее расширение лимитов /ask от 6 октября, 10:18 МСК

Release [`deploy-2026-10-06-ask-limits`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-06-ask-limits), source `19f9ec21d091d59d935b38d727373911b5ea28ca`, прошёл review через [PR #34](https://github.com/Mar2ianen/NedoBot/pull/34). Он установил `ask_llm_max_tokens=16384`, `ask_max_steps=64` (до 68 LLM turns), `ask_action_timeout_sec=180`, `ask_total_timeout_sec=1800`, `ask_max_concurrency=4`, `ask_db_mcp_timeout_sec=30`. Groq остаётся основным; совместимые fallback — отдельные `openrouter_qwen_ask` и `gemini_ask`. Все три ask-профиля используют `thinking=level_high` и timeout 180 секунд. SDK probes артефактом на VPS проверили Groq text/image, OpenRouter text и Gemini image с фактическим бюджетом 16 384; после смены production profile повторены Groq text/image. Профиль ПВО сохранён без изменений, `/ask` в нём выключен.

Артефакт из [release workflow](https://github.com/Mar2ianen/NedoBot/actions/runs/37427751827) установлен **2026-10-06 07:18 UTC / 10:18 МСК**; оба бота и MCP перезапущены. Локальные тесты, formatting, Clippy и GitHub CI прошли. Ledger обеих БД содержал 92 успешные миграции; application errors и missing migrations после рестарта отсутствовали. Предыдущие hashes, artifact и backup paths сохранены в истории этого документа и release metadata.

### Исправления /ask 2026-10-06, 09:30 МСК

Предыдущий production release зафиксирован tag [`deploy-2026-10-06-ask-audit`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-06-ask-audit) на source commit `236e9986333cc3026036365752e764525cce5c86`. Код слит в `main` через [PR #32](https://github.com/Mar2ianen/NedoBot/pull/32); tree merge commit `cbbf4b7c801d` совпадает с tree собранного source. Артефакт из [release workflow](https://github.com/Mar2ianen/NedoBot/actions/runs/37422409833) установлен **2026-10-06 06:30 UTC / 09:30 МСК** в оба community-инстанса; перезапущены `tg-ai-bot-teloxide`, `nedobot-pvo` и `nedonews-mcp`.

Релиз исправляет rich-текст и reply/thread scope `/ask`, точный подсчёт вхождений слов, native tool schemas Gemini, compaction/fallback агента, порядок признаков репутации и voice fallback. В профиле НедоNews в конец `routes.ask.models` добавлен существующий `gemini_flash_comment`; профиль ПВО сохранён без изменений. Миграция `20261006000000_reputation_feature_order.sql` применена в обеих БД: derived materialization v8 использует сохранённый assessment и отсекает устаревшие снимки, без повторного LLM-аудита.

Scoped backfill восстановил текст **52 rich-сообщений НедоNews и 2 ПВО**; повторный dry run в обоих чатах даёт ноль кандидатов. Public MCP возвращает восстановленные сообщения; официальный RMCP client проверил discovery (28 tools) и новый счётчик: 740 вхождений слова «и» в 512 сообщениях за 1–6 октября. SDK проверил native tool calls Groq для текста и изображения с фактическим production-профилем. После restart проверены оба PID и MCP, `NRestarts=0`, успешная миграция, PostgreSQL и оба embedding health endpoints; за шестиминутное окно application `ERROR` и ошибок missing migration нет. Telegram smoke-команды не отправлялись.

SHA256 bot executable этой выкладки: `dd0f428c283029b324b45b4db4736c6f29a3a029cb8f8b2cfa040fac2541751a`; MCP: `2d744c2f84df77e3923e6ea9406c5e35e63d2ba8efd03208ac4294deee55600b`. Проверенный stage и результаты выкладки сохранены в `/opt/tg-ai-bot-releases/deploy-2026-10-06-ask-audit-236e998/`. До выкладки сохранены и проверены custom dumps обеих БД, consistent SQLite backup, binaries, models, `.env` и profiles в приватном `/opt/tg-ai-bot-releases/deploy-2026-10-06-ask-audit-before-20261006T062533Z/`. Эти данные не публикуются.

### Предыдущие выкладки 2026-10-04

Предыдущий release зафиксирован под `deploy-2026-10-04-rep-v2` / `225e59ddf62a` (merge PR #26). Ранее: `deploy-2026-10-04-stats-render` / `d727e0992198`, `deploy-2026-10-04-footer-alias` / `cdfa261f9449`, `deploy-2026-10-04-gemma-head` / `d82d55a083fe`. Следующие записи описывают проверки этих исторических выкладок.

Релиз включает репутационную голову `rep-v2-2026-10-04` (`reputation_enabled=true`): 12 point-in-time фичей, двухпроходной скоринг, слот +4/+8, команды исключены. Проверенный deploy: **2026-10-04 14:05 UTC / 17:05 МСК**, binary `45170162da8bdf562badb99f215f35e0c9b41bce783c03495745c01750a50ec8`. На VPS **`enforce_enabled=false`, `enforce_dry_run=true`**: автоматических банов и удалений нет.

Релиз привозит типизированный рендер статистики (`Section`/`Kv`, строки в `StatsStrings` ru/en по `stats_locale`), сентимент реакций, `/bottommsg`, долю реплаев и сообщений на активного. В прод-профиле локаль не задана — действует `ru` по умолчанию.

Вместе с ним задеплоены алиасы футера канала (`Не теряем связь` + `😎НедоNews`) и `blocked_post_terms` (`#реклама`, `о рекламодателе`): футер-гейт остаётся allowlist, реклама без футера и медиа-посты пропускаются. В прод-профиле алиас и denylist включены.

Предыдущая Gemma-голова `gemma-768-fx-2026-10-04` использовала готовые `telegram_message_embeddings_gemma` вектора. Её заменяет 512d EmbeddingGemma 2 голова; текущий rollout и ограничения датасета будут записаны отдельной записью после production-проверки.

Релиз подключает отдельный `teloxide-antispam v0.3.1` (`0828ad6`), Unicode word/char модель `alt-word-char-v2-2026-10-03` и пороги `alt-word-char-validation-2026-10-03-v1`. Настройки доставки ревью, reviewer/owner, LLM topology, `.env` и секреты сохранены. PVO и public MCP не перезапускались.

Binary собран с `--locked --release` из `79921e49d176`; его Git tree точно совпадает с release commit. SHA256 running executable: `e31dc7d5f5eb9b41ce25d27be7ced64f866d7a89e2c2586b77af26acba9f1d80`; текстовая модель: `b90d9ebae22abde31238269e595164e41025ddf4e0d1c40d5ba1810f387b2d63`; Gemma-голова: `123665884661c0628b2a3ea9ac75e2f69ecb784ab61ec0fb55c3e09636490823`. Metadata — `/opt/tg-ai-bot-teloxide/.release.json`; предыдущие binary/profile сохранены в `/opt/tg-ai-bot-teloxide/backups/deploy-2026-10-04-gemma-head/`. Для rollback исполняемый файл заменять **атомарно через временный файл и rename**, не перезаписывать running executable (Linux ETXTBSY).

Перед выкладкой прошли feature matrix и CI отдельного крейта, CI PR #23, fmt, all-target tests, all-feature Clippy и PostgreSQL integration suite (миграции, audit materialization, manual moderation, MCP). После restart проверены active service, совпадение хеша `/proc/<pid>/exe` с release binary и startup-события загрузки новой модели/calibration. За 35-секундное окно наблюдения application ERROR отсутствовали. Telegram smoke-команды не отправлялись. Датасеты/экспорты/ChatKeeper/прототипы не публикуются; публичны код, методика обучения, aggregate metrics и веса модели. Дальнейшие изменения идут в `dev`, не выдаются за автоматически задеплоенный `HEAD`.

При предыдущем service-messages release проверялись также MCP endpoints (`403` локально / `405` публично); эти проверки не выдаются за повторённые при текущем antispam-deploy. Порядок выкладки и rollback описан в [`docs/DEPLOYMENT.md`](DEPLOYMENT.md).

### Предыдущая dry-run выкладка автомодерации 2026-10-03

Ранее `feat/spam-moderation-backend-v2` (labels writer, `/notspam`, журнал, лестница, LOLS-зеркало, NN-скоринг) выкатывался прямым rsync/build/restart с `enforce_enabled=true`, `enforce_dry_run=true`, ban threshold 90 и v1-моделью. Эта запись историческая: фактические текущие binary/model/flags указаны выше под deployment tag. Цели FPR ≤0.0001% и recall ≥99.9% пока не подтверждены. LOLS-синк настроен ежечасно в :17; отсутствие committed rows/locking остаётся отдельной нерешённой задачей, не гарантией работающего зеркала.



- код: `/opt/tg-ai-bot-teloxide`
- Postgres: Podman container `tg-ai-bot-postgres`
- systemd:
  - `container-tg-ai-bot-postgres.service`
  - `tg-ai-bot-teloxide.service`
  - `nedobot-rag-embedding.service`
  - `nedonews-mcp.service`

PostgreSQL запускается из образа `pgvector/pgvector:0.8.2-pg16-bookworm` на том же persistent volume. Все активные text lanes — memory, unified audit, chat retrieval, локальные `/ask` tools и public MCP semantic search — используют один pinned EmbeddingGemma 2 Q4/512d service на `127.0.0.1:8788`. Image endpoint вызывается только для подтверждённых спамеров. Старые RuBERT 312d и EmbeddingGemma 300M QAT 768d таблицы и unit-конфиги сохранены для rollback, но runtime к ним не обращается. Порты embedding-service наружу не публикуются.

Полезные команды:

```bash
ssh vps-153 'systemctl status tg-ai-bot-teloxide --no-pager'
ssh vps-153 'systemctl status nedonews-mcp --no-pager'
ssh vps-153 'journalctl -u tg-ai-bot-teloxide -f'
ssh vps-153 'podman ps'
```

## Публичный Read-only MCP

Инструмент youtube.get_subtitles — отдельный read-only источник публичных YouTube-субтитров, вне SQL/view surface. Он запускает только абсолютный путь MCP_YOUTUBE_SUBTITLES_COMMAND без shell, принимает прямые HTTPS URL youtube.com/youtu.be, ограничивает число видео, общий объём текста и время выполнения; production-переменные описаны в deploy/nedonews-mcp/nedonews-mcp.env.example.

`https://nedobot.chickenkiller.com/mcp/nedonews/v2` — намеренно публичный MCP Streamable HTTP endpoint с данными только `НедоNews Chat`. Версия в URL отделяет RMCP-контракт от удалённого legacy JSON-RPC API: внешний клиент обязан выполнить `tools/list`, а не переиспользовать старые input/output schemas. Endpoint не даёт ни SQL, ни shell, ни доступ к `public.*`: отдельная PostgreSQL-роль `nedobot_mcp_ro` читает reviewed views `mcp_public` и одну узкую внутреннюю `mcp_private.telegram_media`, используемую только `chat.get_media`.

- Миграция `20260717180000_mcp_public_views.sql` задаёт scope и explicit-колонки. Foreign/private chat scope и raw Telegram API JSON не выдаются как общий доступ; personal-channel поля, явно включённые в `mcp_public`, входят в фактический контракт ниже. Полный reviewed inventory опубликованных view и полей находится в [`MCP_PUBLIC_DATA.md`](MCP_PUBLIC_DATA.md).
- `config/mcp_db_manifest.toml` — проверяемый allowlist views, колонок и их типов, а [`MCP_PUBLIC_DATA.md`](MCP_PUBLIC_DATA.md) — его human-readable snapshot. При старте MCP сверяет manifest с БД и отказывается стартовать при schema drift.
- Внешнему клиенту доступны только структурированные `db.*` и read-only domain tools; значения передаются bind-параметрами, лимит одной страницы — 200, effective column list — 40. Generic page собирается до logical rows budget 480 KiB, затем возвращает корректные `has_more`/`next_cursor`; запас учитывает дублирование RMCP text и structured content в wire response. Широкие views требуют явно передать `columns`. Одно text-поле может содержать до 8192 символов, domain message tools возвращают preview до 4096 символов; при превышении text, JSONB и array поля сообщают `_truncated_fields`, а preview заканчивается `…`. Aggregate `min`/`max` возвращает полное значение либо контролируемую ошибку budget. Соединений с БД — два, `statement_timeout` — 5 секунд.
- `db.search_text` остаётся manifest-инструментом для одной разрешённой текстовой колонки. Domain-инструменты `chat.search_messages`, `chat.search_messages_batch` и `chat.count_messages` используют общий typed search service и доступны одновременно локальному `/ask` RMCP child process и публичному Streamable HTTP router; локальный allowlist ограничивает только `/ask`, а не меняет read-model.
- `chat.search_messages` по умолчанию использует `match_mode: "hybrid"`: русский/simple full-text, fuzzy-сопоставление через `pg_trgm` и semantic similarity по готовым 512d EmbeddingGemma 2 embeddings. Vector leg использует HNSW cosine index (`ef_search=200`, строгий iterative scan) и не является точным или полным поиском; перед ограничением top-1000 к semantic-кандидатам применяются фильтры автора, дат, reply и media. Для semantic leg задан минимальный cosine similarity `0.60`, после чего lexical/fuzzy и semantic score объединяются. Возраст сообщения не меняет semantic score; дата ограничивает выдачу только через явные `date_from/date_to`. Допустимы `full_text`, `any_terms`, `literal` и `whole_word`; для смысловых перефразировок используй `hybrid`, для альтернативных форм — `any_terms`, для точного термина — `whole_word` или `literal`. Если embedding endpoint временно недоступен, hybrid безопасно продолжает lexical/fuzzy search. Результат имеет форму `{messages, total_count, has_more, next_offset, scan_limit_reached}`; для следующей страницы передай возвращённый `next_offset` как `offset` (допустимый диапазон 0–10000). Если достигнут потолок сканирования, `has_more=true`, `next_offset=null`, `scan_limit_reached=true`; поэтому top-k не следует принимать за полный набор.
- `chat.search_messages_batch` выполняет до шести независимых запросов и возвращает метаданные `total_count`/`has_more`/`next_offset`/`scan_limit_reached` для каждого запроса. `chat.count_messages` выполняет отдельный aggregate count с теми же predicates, без сортировки и relevance ranking; поле `query` необязательно и при отсутствии считает все строки по структурным фильтрам. Он возвращает число matching-сообщений, а не число событий или вхождений слова внутри одного сообщения. Поэтому он предназначен для вопросов «сколько сообщений»/«в скольких сообщениях», а не для occurrence count, уникальных авторов или событий по голому «сколько раз»/«как часто».
- Даты принимаются как RFC 3339 или `YYYY-MM-DD`; дата без времени для `date_from` означает начало UTC-дня, а для `date_to` — его конец. По умолчанию поиск исключает сообщения без пользователя, ботов и все пересылки; `include_forwards=true` включает их явно, в том числе строки без сохранённого автора. Результаты доменных message tools содержат `user_id` и явное `author_name` сохранённой строки (поле `author` оставлено backward-compatible alias), а также `is_forwarded` для любой пересылки и `forwarded_from`, если источник удалось извлечь; `is_automatic_forward` остаётся более узким фильтром автоматических пересылок. `chat.get_reply_thread` возвращает JSON-объект с `root_message_id` и массивом `thread`, а не безымянный плоский результат.
- Добавление fuzzy search не меняет `mcp_public` views, scope или sanitization: это только новый read-only query path поверх уже опубликованной проекции. Миграция `20260803090000_chat_search_quality.sql` добавляет `pg_trgm` и индекс для этого пути.
- JSON рекурсивно очищается от ключей наподобие `token`, `secret`, `authorization`, `database_url` и `invite_link`. В логах сохраняются только tool, table/columns/operators, количество строк и latency — без текстов сообщений и значений фильтров.
- `chat.get_media` принимает только ID сообщения публичного чата и возвращает фото или документ размером не более 5 MiB. Удалённые и помеченные как спам сообщения, а также видео, аудио и остальные вложения недоступны. Telegram file IDs остаются во внутренней схеме и не показываются через manifest или generic `db.*` tools. Инструмент выключен по умолчанию; для включения нужны `MCP_MEDIA_ENABLED=true` и отдельная secret-переменная `MCP_TELEGRAM_BOT_TOKEN` в MCP env-файле.

### Public MCP data exposure contract

Это фактический и сознательно принятый контракт экспозиции, а не обещание privacy-minimized projection. Endpoint публичный и намеренно не требует Bearer-токен: локальные процессы входят в доверенную границу управляемого VPS, а внешний MCP публикует только согласованный read-only набор данных. HTTP adapter по умолчанию слушает только `127.0.0.1`; Nginx завершает TLS, ограничивает частоту и число соединений, размер запроса и время ожидания. Проверки Host/Origin ограничивают маршрутизацию и browser-origin, но не являются аутентификацией. Не добавлять application auth без отдельного решения изменить публичный клиентский контракт.

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

Unit `deploy/nedonews-mcp/nedonews-mcp.service` читает только `/etc/nedobot/nedonews-mcp.env`; Telegram token не передаётся, пока выдача медиа выключена. Для включения `chat.get_media` туда отдельно добавляются `MCP_MEDIA_ENABLED=true` и `MCP_TELEGRAM_BOT_TOKEN` из secret store; token не коммитится и не логируется. Для semantic leg `chat.search_messages` в env задаются `ASK_CHAT_EMBEDDING_URL`, `ASK_CHAT_EMBEDDING_MODEL`, `ASK_CHAT_EMBEDDING_TIMEOUT_SEC` и `ASK_CHAT_EMBEDDING_QUERY_PREFIX`; роль MCP получает column-level `SELECT` только на нужные поля `telegram_message_embeddings_gemma2`. MCP использует общий pinned encoder на `127.0.0.1:8788`; старый отдельный GGUF service не входит в активный runtime. Nginx проксирует исключительно `/mcp/nedonews/v2` на `127.0.0.1:8787`, принимает body не больше 64 KiB и ждёт upstream 70 секунд — дольше 60-секундного application deadline.

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
- `post_history_entries` - атомарная история новых постов: строгий Gemma-summary, сущности, использованный ракурс, реально использованный внешний факт и 512d EmbeddingGemma 2 embedding. Исходные посты не склеиваются.
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
- Для первого текстового сообщения сохраняются LLM-маркеры кампании, 512d EmbeddingGemma 2 vector и сходство с вручную подтверждённым спамом. Эти сигналы лишь повышают review-риск; автоматической пометки спамером нет.
- Эмбеддинг первого сообщения персистится в `telegram_new_user_profile_audits.first_message_embedding_gemma2` при материализации аудита и пополняет корпус для будущих `spam_similarity`-проверок; история скоринга при этом не пересчитывается. Backfill запускается бинарём `backfill_audit_embeddings` с deployment-конфигурацией выбранной базы.
- `spammer_avatar_embeddings` содержит 512d image vectors и non-downloadable Telegram file-unique identifiers только для пользователей с явной ручной spam-меткой. Download-capable file id очищается после векторизации, байты изображения не сохраняются; снятие метки удаляет строки набора. Эти примеры пока не участвуют в автоматическом image-классификаторе.
- Template-матчинг (`template_match_count`) сравнивает первое сообщение с текстами из `telegram_messages`, у которых выставлен `spam_marked_at`. Ручная разметка обязана штамповать сообщения (`spam_marked_at`, `spam_source='manual_owner_confirmation'`), иначе помеченные спамеры не попадают в корпус: `is_spammer` на пользователе недостаточно.
- Повтор title личного канала на размеченных спамерах — сильный сигнал reuse (`personal_channel_title_reused_by_spammers`, +24): операторы клонируют фуннель-каналы под каждый аккаунт, chat_id различается, а нормализованный title совпадает. Реюз считается и через `shared_spam_reputation` соседнего инстанса.
- Ротация identity (`identity_display_name_rotation` +12, `identity_username_rotation` +8): смена имён/юзернеймов между profile refresh фиксируется в `telegram_profile_identity_observations`, ротация — маркер операторов.
- LOLS-зеркало (`lols_spam_users`, bin `sync_lols_banlist`, сигнал `lols_spammer_identity` +50): почасовой дамп `lols.bot/spam/banlist.txt` (~3.6M user_id) сворачивается локально через temp swap, lookup в baseline без сети. Замер: 11/11 подтверждённых спамеров в LOLS против 0/8 в CAS.
- CAS (Combot Anti-Spam, `api.cas.chat`) подключён как слабый внешний сигнал за флагом `moderation.cas_enabled` (`cas_timeout_sec`, default 5, валидация 1..30): положительный вердикт даёт не более +12 (`EXTERNAL_SCORE_CAP`) и никогда сам не выводит в high; «Record not found» и любые ошибки трактуются как unknown/clean и ничего не добавляют. Замер на 8 подтверждённых спамерах НедоNews/PVO: покрытие CAS 0/8.
- Лестница автомодерации (`moderation.enforce_enabled=false`, `enforce_dry_run=true`, `enforce_ban_threshold=90`, валидация 50..100, требует `moderation.enabled`): свежий materialize-аудит моложе 24ч — review-порог удаляет первое сообщение (карточка идёт как обычно), ban-порог банит, удаляет до 10 недавних сообщений и пишет System-метку в корпус. Replay старых аудитов никогда не исполняется. Идемпотентность по `is_spammer` и System-метке; снятие бана — вручную `/unban` (решение видно в `moderation_decision_journal`).
- Линейный скоринг первого сообщения (`moderation.linear_spam_enabled=false`, `linear_spam_model_path`, загрузка на старте): TF-IDF + LogReg, инференс в `teloxide_antispam::logreg`. Формат `analyzer="word_12_lower"` сохраняет legacy word 1–2, raw-текст и f32-веса `models/linear_spam_word12_v1.json`; новые правила нормализации к нему не применяются. Обучение v1 описано в `eval/train_linear_spam.py`; сырые локальные тексты не коммитятся. Вероятность — только supporting-вес (p≥0.9 → +18, p≥0.75 → +10), не самостоятельное основание наказания; метрики текстовой модели не равны метрикам всей модерации.
- Новый явно версионированный формат `analyzer="unicode_word_char_v2"`, `preprocessing="unicode-spam-v2"`: NFKC, строгие токен-локальные Latin/Greek↔Cyrillic гомоглифы, обработка invisible/bidi/Zalgo с сохранением emoji ZWJ и обычных й/ё/акцентов, отдельный auxiliary view растянутых слов. Word 1–2 и char_wb 3–5 имеют собственные TF-IDF/L2-блоки; `word` и `character` содержат `vocab` (term→idf), `coef` в лексикографическом порядке терминов и явный `weight`; общий `intercept` и веса — f64. Неизвестные preprocessing/analyzer и лишние feature-блоки отклоняются при загрузке. Для использования v2 нужен отдельный проверенный артефакт и явное изменение deployment-пути; наличие поддержки в коде не переключает production с v1.
- Наблюдения Unicode/форматирования, версия локальной модели и её calibration сохраняются в `risk_first_message_signals` как `first_message_text_observation` с `coefficient=0`, `decision="observation_only"`; они отображаются в карточке ревью **без самостоятельного штрафа**. Низкие вероятности также сохраняются, чтобы аудит не ограничивался положительными срабатываниями. Оригинальный текст, embedding и LLM input не подменяются model view. Материализация имеет версию `unified-audit-materialization-v6` (багфиксы скоринга v0.3.0 плюс Gemma-голова v0.3.1: санитизация similarity/вероятностей, clamp доступного скора, token-local гомоглифы baseline, чтение готовых retrieval-векторов).
- `teloxide-antispam` — самостоятельный [репозиторий](https://github.com/Mar2ianen/teloxide-antispam), релиз `v0.3.0` (`b9d66ff`); бот явно включает `scoring`, `classifier`, `embedding`, `cas`. v0.3.0 фиксит инверсию policy-порогов, санитизацию similarity/вероятностей, clamp скора, token-local гомоглифы baseline и добавляет multilabel-категории (observation-only) плюс версионированные головы поверх замороженных эмбеддингов. Каноникал v2 и веса `alt-word-char-v2-2026-10-03` не менялись. Embedding/category-головы в проде отключены (поля `None`), обучение голов — `tools/train_embedding_multitask.py` в репозитории крейта по приватным adjudicated векторам. Карточка ревью показывает embedding-p и топ-категорию как факты без штрафа.
- Калибровка ALT v2 — **выбор operating points**, не калибровка posterior probability или полного антиспама: p≥0.9 → +10, p≥0.9747144300743944 → +18, иначе 0. Пороги выбраны на validation; 10/18 — прежние supporting-веса, не обученные веса полного пайплайна. Test: 93.87% recall / 2 FP для supporting; 84.50% / 0 FP для strong на 3180 spam / 3305 ham. Ноль FP означает лишь примерно 0.0906% верхнюю FPR при предположении независимости, а не достижение 0.0001%. Candidate selection уже видел test, поэтому нужен новый независимый локальный holdout. Полная [model card](https://github.com/Mar2ianen/teloxide-antispam/blob/v0.2.0/docs/ALT_MODEL_CARD.md) и optional ALT-only модель опубликованы в отдельном релизе; приватные экспорты/ChatKeeper/прототипы не публикуются.
- Gemma-голова `gemma-768-fx-2026-10-04` ([карточка](https://github.com/Mar2ianen/teloxide-antispam/blob/v0.3.1/docs/GEMMA_HEAD_CARD.md), релиз `v0.3.1` крейта): бинарный linear head поверх замороженного EmbeddingGemma-300M-QAT-Q4 (768d, mean, L2, вход с префиксом `title: none | text: `), обучение только на ALT train, пороги только на validation (supporting и strong совпали на p≥0.9: test 95.50% recall / 0 FP). Fusion с TF-IDF (offline, равные веса): 98.52% / 2 FP и 95.38% / 0 FP. Бот читает готовые `ready`-векторы из `telegram_message_embeddings_gemma` с проверкой `embedding_model`; нет вектора — нет сигнала, новых inference-запросов ноль. Конфиг: `moderation.embedding_spam_enabled` + `embedding_spam_model_path` (absolute path, fail-fast без пути). Оговорки те же: test переиспользован, ALT≠прод, сборка энкодера train/prod различается в 4-м знаке косинуса, цели не достигнуты.
- Репутация `rep-v2-2026-10-04` ([карточка](https://github.com/Mar2ianen/teloxide-statistics/blob/v0.2.0/docs/REPUTATION_MODEL_CARD.md)): 12 point-in-time фичей (ID-приор, риск аудита, активность, сентимент реакций, стэкинг текстовых вероятностей), 3382 adjudicated пользователя (183 спама после owner-разбора очереди несогласий, команды исключены). CV ROC 0.9925 / PR 0.9155; на recall тупого regex (0.306) — 0 FP против 140. Слот +4/+8, никогда не решающий. Двухпроходной скоринг: `audit_risk`-фича — дорепутационный тотал, как в снапшотах обучения. Материализация `v8`: признаки выбираются по именам artifact, Gemma и TF-IDF не переставляются; `has_text` отражает наличие текста. Пропуск behavior snapshot отключает supporting signal.
- Для оценки классификатора нужны отдельно проверенные метки сообщений: неразмеченный экспорт не равен ham, бан аккаунта не делает каждое его сообщение spam, а вердикт чужого детектора не является gold-разметкой. Split должен исключать пересечения по авторам/шаблонам и учитывать время; синтетическая устойчивость к гомоглифам не доказывает целевую FPR автомодерации. Нулевая ошибка на небольшой выборке также не подтверждает FPR ≤0.0001%.

`spam_review_requests` — сохраняемый read-model ревью-аудита, а не гарантия
отправки карточки каждому новому пользователю. Маршрут, включение доставки,
reviewer/owner и risk profile задаются конфигурацией инстанса. В текущем
delivery-path сравниваются актуальные `risk_score` и сохранённый `review_threshold`;
фиксированные `70` и `@Chechulinm` не являются универсальным контрактом.
Проверки выполняются при claim и финализации доставки; выключенная доставка
подавляет pending-уведомления, не отключая сбор аудита.
Поздние сигналы аватара или первого сообщения могут сделать уже сохранённый audit
доставляемым. Кнопки «Верно: спамер» и «Неверно: не спамер» доступны только
авторизованным reviewer/owner согласно moderation-конфигурации; решение закрывает запрос и убирает
клавиатуру. Технические labels риска в карточке переводятся в понятные причины.
Все решения пишутся через единый writer `features::labels`: событие в
`spam_label_events` + флаги пользователя + штампы сообщений. Без события
reuse-сигналы и template-корпус помеченного не видят. Команда `/notspam`
reply (reviewer: configured reviewer, owner или админ чата) пишет `not_spam`
и снимает пометку — так собираются confirmed-ham примеры для классификатора.
Репорты пользователей:

- `/report [причина]` принимается только как reply на сообщение человека в основном чате. Сообщения ботов, автоматические пересылки, личные чаты и команды без reply отклоняются до записи в БД.
- `telegram_reports` имеет уникальность `(chat_id, message_id)`: одно сообщение можно отправить на review только один раз, даже если его пытаются репортить разные пользователи.
- Для защиты от флуда один репортёр может создать не более одного нового репорта за 10 минут. Повтор того же сообщения возвращает существующий репорт и не расходует лимит.
- Репорт хранит snapshot автора и сообщения, причину, media/reply-контекст и подтягивает к rich-карточке текущие профильные, activity и антиспам-сигналы. Сырые данные репортов не добавляются в `mcp_public` и не публикуются через внешний MCP.
- Карточка отправляется отдельным durable delivery-воркером в личные сообщения текущих администраторов чата и owner fallback. Telegram не позволяет боту написать администратору, который ни разу не открыл личку с ботом: такие доставки помечаются `unreachable`, остальные ретраятся с lease и backoff.
- В карточке используются typed Rich Message blocks, ссылка на исходное сообщение и inline-клавиатура: `Спам` и `Не спам`. Callback повторно проверяет актуальное админство; решение атомарно фиксируется в `telegram_reports`, после чего кнопки действий убираются. Кнопки не банят и не удаляют сообщения автоматически; подтверждённые решения штампуются в корпус меток (`manual_owner_report` / confirmed-ham).


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
/bottommsg [-r|-p]
/userstats <id|username> [-r|-p]
/userstatus <id|username> [-r|-p]
/mute [duration] [reply|id|@username ...] [-- optional reason]
/ban [duration] [reply|id|@username ...] [-- optional reason]
/warn [duration] [reply|id|@username ...] [-- optional reason]
/unmute [reply|id|@username ...]
/unban [reply|id|@username ...]
/unwarn [reply|id|@username ...] [#warning-id|all]
/warns [reply|id|@username]
/modlog [reply|id|@username] [limit]
/undo
/report [причина]  (reply на сообщение человека)
```

В группах лучше писать с username:

```text
/ping@nedostraj_bot
```

`/stats_day`, `/stats_week` и `/stats_month` показывают имена пользователей как скрытые ссылки на Telegram-профиль, без видимого ID. Рядом выводятся короткие бейджи: `админ`, `в чате`, `не в чате`, `бот` или `статус неизвестен`.

`/userstats` принимает числовой Telegram ID, уже виденный ботом username или reply на сообщение пользователя. Без аргумента команда показывает отправителя. `UserStatsArgs` один раз нормализует command arguments: render-флаги `-r`/`--rich` и `-p`/`--plain` можно поставить до или после target, они не считаются частью username, а команда только с флагом сохраняет reply/sender fallback. Нормализованный target используется и для refresh профиля, и для построения отчёта. В общих отчётах ID намеренно не печатается; для точного SQL-разбора он остаётся в таблицах `telegram_messages`, `telegram_user_profiles` и `telegram_chat_users`.

Ручные команды модерации доступны только в отдельной Cargo feature `manual-moderation`, выключенной по умолчанию. Дополнительно нужны `[manual_moderation].enabled = true` и `chats.<name>.manual_moderation = true`; для Недобота все эти настройки остаются `false`. Причина необязательна и передаётся через `-- причина`. `/mute` по умолчанию действует сутки, `/ban` — бессрочно, предупреждение активно 30 дней; три активных предупреждения создают mute на пять дней. Команды требуют администратора чата; операции ограничения также проверяют право бота ограничивать участников. Цели — reply, Telegram ID или известный чату username. `/userstats` показывает активную санкцию и число активных предупреждений, а `/warns` и `/modlog` доступны только администраторам.

Уведомления Telegram о входе и выходе участников можно автоматически удалять для отдельного чата через `chats.<name>.delete_join_leave_messages = true`. По умолчанию настройка выключена; боту нужно право `can_delete_messages`. Ошибка удаления не прерывает сохранение и обработку update.

Приветствие и прощание настраиваются шаблонами `chats.<name>.welcome_message` и `chats.<name>.farewell_message`. Оба параметра по умолчанию отсутствуют. События берутся из `chat_member`; для их получения бот должен быть администратором чата. Чтобы заменить стандартные уведомления Telegram своими шаблонами, включи `delete_join_leave_messages`. В шаблонах можно использовать `{first_name}`, `{last_name}`, `{full_name}`, `{username}`, `{user_id}` и `{chat_title}`. Значения пользователя и чата автоматически экранируются для HTML; сам шаблон допускает HTML-разметку. Боты не получают приветствия и прощания.

`chats.<name>.ephemeral_command_replies = true` отправляет поддерживаемые ответы команд как Telegram ephemeral: их видит автор команды и бот, остальные участники группы их не видят. Эта настройка распространяется на ответы ручной модерации, статистики, репортов, заметок и короткие служебные команды; длинный ответ `/ask`, результаты `/transcribe` и обычные сообщения фоновых задач остаются публичными. В forum topics сохраняется исходная тема, хотя ephemeral-ответ не цитирует публичную команду. Если у команды нет обычного пользовательского автора, ответа не будет; он не станет публичным. Для ephemeral бот должен быть администратором чата; Telegram не гарантирует доставку, поэтому ошибка отправки не переключает приватный ответ на публичный.


## Prompt

### Chat retrieval (shadow rollout)

Gemma строит единый `ResearchPlan`: главный subject/audience, secondary context, chat semantic/lexical queries и запросы к внешним источникам. Shadow retrieval объединяет EmbeddingGemma 2 512d vector, PostgreSQL full-text и безопасные literal-regex совпадения за 30 дней с geometric freshness. Кандидаты и ограниченные ветки сохраняются в `chat_research_runs`, но не меняют комментарий без ручной проверки.

`CHAT_AUTHOR:id` и `CHAT_MESSAGE:id:label` разрешены только для ID из подтверждённого retrieval-контекста. Имя автора берётся только из `first_name`; при username ссылка ведёт на профиль, иначе на сообщение. Без evidence обязателен обычный `CHAT_LINK`.

Основной prompt лежит в [prompts/first_comment.md](../prompts/first_comment.md).
Короткий факт-чек/RAG для защиты от устаревших утверждений лежит в [prompts/tech_rag.md](../prompts/tech_rag.md).
Cleanup prompt для расшифровки голосовых лежит в [prompts/voice_cleanup.md](../prompts/voice_cleanup.md).

Модель первого комментария возвращает structured JSON: `{"comment":"...","used_search_result_id":null}`. В `comment` обязателен ровно один `{CHAT_LINK}` или вариант с разрешённым текстом ссылки вроде `{CHAT_LINK:чате}` / `{CHAT_LINK:комментах}`. Gemini получает JSON Schema через API, Ollama fallback — ту же schema через `format`; для остальных совместимых провайдеров сохраняется строгий JSON-контракт в prompt.

Если поиск вернул безопасный результат с публичным HTTP(S) URL, модель обязана выбрать один отдельный угол, которого нет в новости: связанный релиз, ограничение, последствие, сравнение, цену, changelog или реакцию сообщества. Поиск нельзя использовать только для подтверждения или пересказа факта из поста. `used_search_result_id: null` допускается только при пустом или небезопасном поиске. One-based ID сохраняется в `llm_generations`, а `{SOURCE_LINK:N:подпись}` становится обязательным. Подпись должна быть частью фразы («как пишет VideoCardz»), а не отдельным «детали» или «источник». `COMMENT_BLOCKED_SOURCE_DOMAINS` исключает указанные домены и поддомены до fetch, из prompt и при финальном рендере; `COMMENT_BLOCKED_TERMS` так же исключает результаты и комментарии с заданными фрагментами текста. Search response сохраняется до best-effort fetch: неуспешный fetch не удаляет title/snippet уже найденного источника. Output validator отклоняет факт без источника, raw URL, битые/лишние плейсхолдеры, неподходящий ID, текст длиннее 180 видимых символов и generic CTA. Код сам рендерит ссылки в HTML, а предпросмотр ссылок отключён для обычных и rich text send-путей.
RAG не предназначен для пересказа новости: пост канала важнее, а карточки нужны только чтобы не писать ложные вещи вроде `Switch 2 еще не вышла`.

Автоматическая история работает поверх EmbeddingGemma 2 Q4 и pgvector:

- после успешной отправки комментария создаётся отдельная job с исходным постом, комментарием бота и только реально выбранным результатом поиска;
- Gemma получает строгую JSON Schema через provider API и возвращает `summary`, `entities`, `used_angle`, `external_fact`, `skip_reason`;
- `summary: null` разрешён для рекламы, мемов, служебных публикаций, повторов и постов без устойчивого полезного факта; запись становится `ignored` и не участвует в retrieval;
- полезная запись получает усечённый и перенормированный 512-мерный embedding EmbeddingGemma 2 и становится `ready`;
- retrieval истории и recent-comment anti-repeat ограничены парой `discussion_chat_id` + `source_channel_id` текущей job; другие маршруты не попадают в контекст комментария;
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
10. Сохранить raw ASR text, segments и raw JSON; при включённом shadow-ASR дополнительно сохранить независимые альтернативные транскрипты.
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
primary: Gemini Files API + Interactions API
model = runtime.voice_asr_model
language_codes = runtime.voice_language as BCP-47
mode = verbatim

secondary: Groq OpenAI-compatible audio transcriptions
model = runtime.voice_asr_shadow_model
response_format = verbose_json
```

Cleanup request:

- сначала используется profile route `voice_cleanup` и его fallback chain;
- при включённом shadow-ASR cleanup получает основной Gemini-текст и отдельный
  альтернативный Groq-текст; альтернативы используются только для сверки
  спорных слов и не становятся самостоятельным источником новых фактов;
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

### Целевые метрики антиспама

Таргет, к которому идём (зафиксирован 2026-10-03, неделя наблюдения в dry-run):

- False positive rate ≤ 0.0001% — легитимный пользователь практически никогда не должен получать enforcement. Операционно: 0 подтверждённых ложных банов/удалений в неделю; каждый спорный кейс разбирается через `moderation_decision_journal` и `/notspam`.
- Spam detection rate ≥ 99.9% — пропущенный спам измеряется по review-очереди (`risk_score < review_threshold`, но подтверждённый спам задним числом) плюс ручной досмотр выборки.

Замер недели: dry-run решения (`enforce_dry_run=true`) логируются, review-карточки идут как обычно; в конце недели сверяем dry-run ban/delete против фактически подтверждённых (`spam_label_events`) и считаем precision/recall лестницы.

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

Отчёт охватывает `first-comments`, `embeddings`, `post-history` и `reviews`: число jobs и суммарные attempts по статусу, `oldest_ready_age` для старейшей due initial/retry job, безопасные группы `error_kind` с attempts и terminal failures, а также суммарный `lease_reclaim_count`. Expired processing leases не входят в ready-age. Для reviews predicate совпадает с ready-частью production claim: `status = pending`, `risk_score >= review_threshold`, notification `pending/retry_wait` и due time. Неизвестный persisted error kind не выводится: он агрегируется как `other`. Для embeddings отдельно показан текущий счётчик rows с `embedding_batch_cardinality`.

`lease_reclaim_count` сохраняется в доменной таблице и увеличивается только когда worker действительно забирает просроченную `processing` lease. Обычный claim из `pending`/`retry` и повторная попытка после явной failure-finalization его не увеличивают. Для reviews используется аналогичное поле `notification_lease_reclaim_count` её delivery lifecycle.

### Preflight optional index для просроченных spam-review leases

Существующий partial index `spam_review_requests_notification_ready_idx` обслуживает due `pending`/`retry_wait` reviews. Отдельного индекса для reclaim ветки `notification_status = 'processing' AND notification_lease_expires_at <= now()` сейчас нет намеренно: добавлять migration следует только после production evidence, а не заранее.

На production сначала снять размер очереди и число реально reclaimable leases (в read-only сессии):

```sql
select
    count(*) as total_reviews,
    count(*) filter (
        where status = 'pending'
          and risk_score >= review_threshold
          and notification_status = 'processing'
          and notification_lease_expires_at <= now()
    ) as expired_processing_ready,
    count(*) filter (
        where status = 'pending'
          and risk_score >= review_threshold
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
  and risk_score >= review_threshold
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
  and risk_score >= review_threshold
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
- EmbeddingGemma 2 Q4 работает на CPU и оценивает смысловую близость; окончательное решение о полезности карточки и направлении поиска остаётся за Gemma.
- Реакции считаются только с момента включения reaction updates; старые реакции Telegram Bot API задним числом не отдаёт.
- Статусы пользователей известны по последнему `chat_member` update или по будущим снимкам; если Telegram не присылал событие, статус будет `unknown`.
- Если LLM provider вернёт ошибку/subscription limit, задача может остаться без комментария до ручного вмешательства.
- Voice ASR работает в двойном режиме Gemini primary + Groq shadow; local Whisper/Ollama audio не подключены.
- Cleanup provider/model для voice пока не сохраняются в отдельные DB-поля, хотя поля в таблице уже есть.
- Join-конверсия по отдельной invite-ссылке пока не считается автоматически.
- Админки пока нет; статические настройки меняются в `[runtime]` profile TOML и требуют рестарта сервиса. DB-backed dynamic policy — отдельный следующий этап.

## /ask: контекст и аудит native tasks

System prompt вшит из `prompts/ask.md`; журнал выпущенных возможностей хранится в `docs/BOT_CHANGELOG.md` и подмешивается в него только для вопросов о самом боте. Оба файла включаются через `include_str!`, поэтому после их правки нужен rebuild. Правило ведения журнала описано в `DEVELOPMENT.md`. Typed rich-проекция используется для истории и reply, включая nested blocks, таблицы и captions; служебные metadata и thinking исключены. В group scope берётся ID текущего чата, в private scope — настроенный default chat. Перед ответом на reply агент подтягивает bounded MCP context и записывает его в audit.

Пользовательский ввод и каждый результат инструмента передаются отдельными сериализованными JSON-объектами; потенциальные разделители внутри данных экранируются Unicode escapes, а system prompt задаёт все значения как недоверенные данные. Это защищает границы контекста, но не заменяет scope-фильтры: reply-запрос ограничен target и цепочкой родителей, а ссылки строятся только по наблюдавшимся ID. Опциональный `runtime.ask_python_sandbox_enabled` добавляет `sandbox.python`; ему требуется локальный immutable Podman image ID `sha256:<64 hex>` в `runtime.ask_python_sandbox_image` и отдельный rootless пользователь `nedobot-sandbox`. Каждый запуск — новый контейнер с отключённой сетью, read-only rootfs, снятыми capabilities, no-new-privileges, стандартным seccomp, лимитами 1 CPU / 256 MiB RAM без swap / 32 MiB временного `/workspace` / 4 MiB на файл и timeout 15 секунд. Код и максимум один UTF-8 текстовый документ из reply (табличные данные, обычный текст и исходный код; до 2 MiB) передаются через stdin. Документ сохраняется только во временном `/workspace` контейнера; содержимое, Telegram file ID и ключи не пишутся в audit или логи. Окружение процесса очищено; до четырёх запусков на `/ask`. Запуски независимы, Python-переменные и любые созданные файлы удаляются после вызова, поэтому это не постоянное Jupyter-ядро и файлы-результаты нельзя скачать отдельно. Фича выключена по умолчанию.

`chat.count_messages` возвращает число сообщений с `unit=messages`. `chat.count_word_occurrences` возвращает число неперекрывающихся вхождений точного слова/фразы и matching_messages, `unit=word_occurrences`. Runtime требует предварительный поиск с теми же фильтрами; count без query разрешён для structural count. Результаты count сохраняются при сокращении контекста. Native tool pairs удаляются целиком, per-turn previews ограничены, provider-specific continuation ID не используется с полной историей. После action timeout повтор начинается со следующего совместимого profile. `ask_runs.provider/model` отражают реально ответивший profile; `step_count` включает model turns без tools.

У `routes.ask` есть Gemini vision fallback, независимый от текстовых profiles. `probe_ask_route [--image]` проверяет native tool call без Telegram/БД. `backfill_rich_messages <chat-id> [--apply]` восстанавливает старые rich-тексты; без `--apply` делает dry run. См. [аудит](ASK_AUDIT_2026-10-06.md) и [выкладку](DEPLOYMENT.md).

Legacy ASR profiles не включают comparative shadow автоматически. Для Gemini primary видео-кружки используют Groq media fallback и настроенный `voice_asr_shadow_model`, даже при выключенном comparative shadow; Groq key обязателен. ASR errors очищаются от URL, Gemini request chain ограничена 420 секундами, cleanup — 10 секундами; смена job phase продлевает lease.
