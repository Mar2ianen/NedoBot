# TG AI Bot Teloxide

Telegram-бот на Rust/teloxide для `НедоNews Chat`.

Текущая MVP-задача: бот помогает живому Telegram-чату не терять контекст. Основные контуры: первый комментарий под постом канала, память/RAG для новостей, статистика чата и расшифровка голосовых через Groq ASR + LLM cleanup.

## Что Уже Работает

- Читает сообщения из `НедоNews Chat`, если privacy mode выключен до добавления бота в чат.
- Сохраняет входящие сообщения в Postgres.
- Распознаёт авто-форварды из канала по `forward_origin.channel.id`.
- Пропускает рекламу/служебные посты без маркера `Не теряем связь`.
- Скачивает самое большое фото поста и отправляет его в vision-модель, если текущий LLM provider/model поддерживает изображения.
- Генерирует комментарий через LLM provider router: `ollama`, `groq`, `cerebras`, `openrouter`, `openai_compat`.
- Отправляет HTML-комментарий reply под постом.
- Отключает link preview.
- Подставляет premium/custom emoji по тематике, включая канал/AMD/Radeon/Ryzen.
- Пишет задачи и результаты генерации в Postgres.
- После комментария асинхронно создаёт атомарную Gemma-карточку полезного поста; рекламу, мемы и повторы помечает `ignored`.
- Ищет релевантную историю через RuBERT Tiny 2 и pgvector с отдельными similarity, temporal coefficient и итоговым rank score.
- Подмешивает последние ответы бота в prompt, чтобы не повторять одинаковые CTA.
- Опционально добавляет свежий web/GitHub/Reddit факт-чек для первого комментария через lazy MCP process, если включён `SEARCH_ENABLED`.
- Собирает статистику чата с дневной/недельной/месячной отсечкой в 05:00 МСК.
- Показывает пользователей в отчётах человекочитаемо: имя кликабельно, ID спрятан в `tg://user`, рядом статус/админство.
- Сохраняет новые reaction updates, reaction count updates и chat member updates, если Telegram отдаёт их боту.
- Расшифровывает `voice`, `audio` и `video_note`, если включены `VOICE_TRANSCRIPTION_ENABLED` и `VOICE_AUTO_TRANSCRIBE`.
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

Локальный `.env` не коммитится. Безопасные примеры и полный перечень переменных приведены в этом разделе; секреты задаются только в локальном окружении или в защищённом server-side environment file.

Основные переменные:

```env
TELOXIDE_TOKEN=
DATABASE_URL=postgres://tg_ai_bot:tg_ai_bot@localhost:5432/tg_ai_bot

SOURCE_CHANNEL_ID=-1001575496091
DISCUSSION_CHAT_ID=-1001932061163
CHAT_INVITE_URL=https://t.me/+RxmPtw7Bs-IxNzEy
CHAT_INVITE_LABEL=Присоединяйтесь к чату
POST_SIGNATURE_MARKER=Не теряем связь

# Optional: enables strict profile-authoritative task routing.
# LLM_PROFILES_PATH=/etc/tg-ai-bot/llm_profiles.toml
LLM_PROVIDER=gemini
LLM_MODEL=gemini-3.5-flash
LLM_SUPPORTS_IMAGES=true
LLM_TEMPERATURE=0.45
LLM_MAX_TOKENS=180
LLM_PROXY_URL=
MEMORY_LLM_TEMPERATURE=0.2
MEMORY_LLM_MAX_TOKENS=220
MEMORY_LLM_PROVIDER=ollama
MEMORY_LLM_MODEL=gemma4:31b
RAG_ENABLED=true
RAG_EMBEDDING_URL=http://127.0.0.1:8788
RAG_EMBEDDING_MODEL=cointegrated/rubert-tiny2
RAG_EMBEDDING_TIMEOUT_SEC=10
RAG_TOP_K=6
RAG_MIN_SIMILARITY=0.55
RAG_TEMPORAL_HALF_LIFE_DAYS=180

SEARCH_ENABLED=false
SEARCH_EXTRACT_PROVIDER=ollama
SEARCH_EXTRACT_MODEL=gemma4:31b
SEARCH_EXTRACT_TEMPERATURE=0.1
SEARCH_EXTRACT_MAX_TOKENS=900
SEARCH_MCP_COMMAND=
SEARCH_MCP_ARGS=
SEARCH_MCP_ENV=
SEARCH_MCP_TIMEOUT_SEC=8
SEARCH_QUERY_TIMEOUT_SEC=20
SEARCH_MCP_TOOL_WEB=web_search
SEARCH_MCP_TOOL_GITHUB=github_search
SEARCH_MCP_TOOL_REDDIT=reddit_search
SEARCH_MCP_TOOL_FETCH=web_fetch_exa
SEARCH_FETCH_TOP_N=4
SEARCH_FETCH_MAX_CHARS=16000
CHAT_RETRIEVAL_EMBEDDINGS_ENABLED=false
CHAT_RETRIEVAL_SHADOW_ENABLED=false
CHAT_RETRIEVAL_EVIDENCE_ENABLED=false
CHAT_RETRIEVAL_EVIDENCE_MIN_SCORE=2.0
SEARCH_GITHUB_MCP_COMMAND=
SEARCH_GITHUB_MCP_ARGS=
SEARCH_GITHUB_MCP_ENV=PATH,HOME,GITHUB_PERSONAL_ACCESS_TOKEN
SEARCH_GITHUB_MCP_TOOLS=search_issues,search_code

GROQ_API_KEY=
GROQ_MODEL=
CEREBRAS_API_KEY=
CEREBRAS_MODEL=
# Unified audit rollout: shadow first, then authoritative cutover.
NEW_USER_AUDIT_ENABLED=false
NEW_USER_AUDIT_AUTHORITATIVE_ENABLED=false
NEW_USER_AUDIT_MAX_TOKENS=900
AVATAR_CLASSIFIER_ENABLED=true
FIRST_MESSAGE_SPAM_ENABLED=false
OPENROUTER_API_KEY=
OPENROUTER_MODEL=
GEMINI_API_KEY=
GEMINI_TEXT_MODEL=gemini-3.6-flash
GEMINI_FLASH_MODEL=gemini-3.5-flash
GEMINI_FLASH_LITE_MODEL=gemini-3.5-flash-lite
GEMINI_LEGACY_FLASH_LITE_MODEL=gemini-3.1-flash-lite
GEMINI_TTS_MODEL=gemini-3.1-flash-tts-preview
GEMINI_THINKING_BUDGET=1024

PUBLIC_BASE_URL=https://nedobot.chickenkiller.com
STATIC_FILES_DIR=/opt/tg-ai-bot-teloxide/static
LLM_PROXY_URL=
OLLAMA_API_KEY=
OLLAMA_BASE_URL=https://ollama.com
OLLAMA_MODEL=gemma4:31b
VISION_MODEL=gemma4:31b
OPENAI_COMPAT_API_KEY=
OPENAI_COMPAT_BASE_URL=https://api.openai.com/v1
OPENAI_COMPAT_MODEL=

OWNER_TELEGRAM_ID=
SEND_OWNER_PREVIEW=true
ASK_ENABLED=false
ASK_MAX_STEPS=7
ASK_ACTION_TIMEOUT_SEC=45
ASK_TOTAL_TIMEOUT_SEC=180
ASK_MAX_CONCURRENCY=1
ASK_DB_MCP_TIMEOUT_SEC=8
PROFILE_REFRESH_CONCURRENCY=4
```

`nedobot.chickenkiller.com` — публичный HTTPS-домен проекта. Он отдаёт только
кэшированные аватарки Telegram по пути `/tg-ai-bot-static/avatars/`; бот строит
их URL из `PUBLIC_BASE_URL`. Production-конфиг общего SNI-фронта лежит в
`deploy/vpn-nginx/nginx.conf`; сертификат Let’s Encrypt обновляется Certbot, а
deploy hook перезагружает контейнерный Nginx после продления.

Для комментариев рекомендуемый основной provider — `gemini`: `Gemini 3.6 Flash` как основная модель, затем `Gemini 3.5 Flash`, `Gemini 3.5 Flash Lite`, `Gemini 3.1 Flash Lite` и в конце `ollama`/`gemma4:31b`. Fallback-цепочка срабатывает только когда модель не переопределена явно на уровне конкретного вызова.

### Строгие LLM profiles

`LLM_PROFILES_PATH` необязателен. Если он отсутствует, генерация и стартовые проверки сохраняют legacy-поведение `LLM_PROVIDER`/моделей. Если переменная указывает на TOML-файл profiles, режим строгий: каждая генерация использует свой task route (`first_comment`, `memory`, `voice_cleanup`, `search_extract`, `avatar_analysis`, `first_message_spam`, `new_user_audit` или `ask`) и игнорирует legacy provider/model overrides. Выбранная модель route задаёт driver, base URL, model ID, capabilities, request timeout и `api_key_env`.

На старте каждый включённый route разрешается с его фактическими требованиями к изображению, system prompt и числу output tokens. Для каждого совместимого fallback selection проверяется заданная secret env-переменная; ошибка называет только имя переменной, но не её значение. `structured_output = "prompt_only"` намеренно не передаёт OpenAI-compatible `response_format`: JSON-контракт остаётся в prompt и проверяется typed output validator. Полная topology приведена в `config/llm_profiles.toml.example`.

Если Gemini недоступен напрямую из региона сервера, `LLM_PROXY_URL` может направить только LLM/Gemini-запросы через HTTP/SOCKS proxy, не трогая Telegram polling. На текущем `vps-153` Gemini-трафик идёт через `LLM_PROXY_URL=socks5h://127.0.0.1:2080`, который поднимает systemd-сервис `gemini-proxy-ssh.service` SSH-туннелем до `vps-85`.

Для Gemini 3.x бот использует актуальный `thinkingLevel=low` и не передаёт устаревшие `temperature` и числовой `thinkingBudget`. `LLM_MAX_TOKENS` задаёт полный лимит вывода; для JSON-комментария нужен запас, поэтому значение по умолчанию — 180. Для старых Gemini-моделей сохраняется `GEMINI_THINKING_BUDGET`: бот отправляет `maxOutputTokens = LLM_MAX_TOKENS + GEMINI_THINKING_BUDGET`.

На старте основной сервис и `retry_pending_comments` делают fail-fast проверку секретов для включённых функций:

- `LLM_PROVIDER=gemini` требует непустой `GEMINI_API_KEY` или `GOOGLE_AI_STUDIO_API_KEY`.
- `LLM_PROVIDER=groq|cerebras|openrouter|openai_compat` требует соответствующий API key.
- `LLM_PROVIDER=groq|cerebras|openrouter` требует явную модель через `LLM_MODEL` или provider-specific переменную `GROQ_MODEL`/`CEREBRAS_MODEL`/`OPENROUTER_MODEL`; fallback на `VISION_MODEL` запрещён.
- `LLM_PROVIDER=ollama` секрета не требует.
- Если включены `VOICE_TRANSCRIPTION_ENABLED=true` и `VOICE_AUTO_TRANSCRIBE=true`, `VOICE_ASR_PROVIDER=groq` требует `GROQ_API_KEY`.
- Если для включённого voice pipeline задан `VOICE_CLEANUP_PROVIDER`, для него тоже проверяется соответствующий LLM secret.
- `NEW_USER_AUDIT_ENABLED=true` запускает unified worker и использует существующий LLM provider/profile без отдельных secret/model переменных. `NEW_USER_AUDIT_MAX_TOKENS` ограничивает его output и по умолчанию равен `900`. При `NEW_USER_AUDIT_AUTHORITATIVE_ENABLED=false` worker сохраняет shadow assessments, а legacy pipelines остаются источником истины. Authoritative cutover требует одновременно `NEW_USER_AUDIT_ENABLED=true`, `AVATAR_CLASSIFIER_ENABLED=false` и `FIRST_MESSAGE_SPAM_ENABLED=false`; он также требует корректные `RAG_EMBEDDING_URL`, `RAG_EMBEDDING_MODEL` и `RAG_EMBEDDING_TIMEOUT_SEC`, поскольку материализация оценивает embedding первого сообщения.

Это специально ловит ситуацию, когда конфиг переключили на Gemini, но ключ на сервере пустой: бот не стартует с тихим уходом в fallback.

`/ask` использует два независимых deadline: `ASK_ACTION_TIMEOUT_SEC` ограничивает одну генерацию действия LLM (с одной retry-попыткой после timeout), а `ASK_TOTAL_TIMEOUT_SEC` ограничивает исследование целиком, включая MCP и внешние tools. Значения `0` запрещены. Старый `ASK_TIMEOUT_SEC` временно поддерживается только как совместимый alias для action timeout, пока production environment files переезжают на явное имя.

### Поиск фактов для первого комментария

SEARCH-контур добавляет вспомогательный свежий контекст перед генерацией первого комментария:

```text
clean post -> extract JSON queries -> lazy MCP process -> SearchContext -> build_llm_prompt -> generate_text_checked
```

Поведение gated by config:

- `SEARCH_ENABLED=false` сохраняет старое поведение: search-блок не добавляется в prompt, а генерация идёт через обычный `LLM_PROVIDER` без внешнего поиска.
- `SEARCH_EXTRACT_PROVIDER` / `SEARCH_EXTRACT_MODEL` задают LLM, который из очищенного поста возвращает JSON с максимум 4 запросами для `web`, `github` или `reddit`.
- `SEARCH_MCP_COMMAND` и `SEARCH_MCP_ARGS` запускают основной MCP server лениво на один search-run. Long-lived MCP client в `AppState`, lifecycle restart/shutdown и постоянный child process не используются в первой итерации.
- `SEARCH_MCP_ENV` — allowlist имён env vars, которые можно передать MCP child process. Значения не логируются.
- `SEARCH_QUERY_TIMEOUT_SEC` — отдельный deadline одного source query. Таймаут GitHub, Reddit или web не отбрасывает результаты остальных источников.
- `SEARCH_MCP_TOOL_WEB`, `SEARCH_MCP_TOOL_GITHUB`, `SEARCH_MCP_TOOL_REDDIT` задают имена MCP tools для основного MCP server.
- `SEARCH_MCP_TOOL_FETCH` включает дополнительный fetch top URL после search. Для Exa это `web_fetch_exa`.
- `SEARCH_GITHUB_MCP_COMMAND` / `SEARCH_GITHUB_MCP_ARGS` включают отдельный GitHub MCP server для запросов `source=github`; если они не заданы, GitHub-запросы идут через основной `SEARCH_MCP_TOOL_GITHUB`.
- `SEARCH_GITHUB_MCP_ENV` по умолчанию пропускает только `PATH,HOME,GITHUB_PERSONAL_ACCESS_TOKEN`; значения не логируются.
- `SEARCH_GITHUB_MCP_TOOLS` по умолчанию вызывает только read-only `search_issues,search_code`; write tools GitHub MCP не вызываются.
- Для GitHub results бот дополнительно дочитывает top-N URL через read-only `get_issue` / `get_file_contents`: issue/PR body, `README.md`, `CHANGELOG.md`, release docs и другие blob-файлы попадают в snippet как `Fetch: ...`.
- `SEARCH_FETCH_TOP_N` ограничивает число URL для fetch, `SEARCH_FETCH_MAX_CHARS` — объём текста на страницу.
- Ошибка extract превращается в skipped `SearchContext`; ошибка или таймаут отдельного MCP source оставляет успешные результаты других источников доступными для комментария.
- Результаты поиска добавляются в JSON-контекст без raw URL и имеют приоритет ниже текста поста. В промпт помещается до 24 результатов, до 16 000 символов на результат и до 160 000 символов суммарно; URL остаётся только в `SearchContext` для безопасного рендера.
- Каждый search-run сохраняется в `search_runs` для аналитики: статус, skipped reason, latency, queries/results как `jsonb`. Кэша результатов пока нет — запись аналитическая, не влияет на генерацию.
- Chat retrieval работает отдельно: `CHAT_RETRIEVAL_SHADOW_ENABLED` сохраняет гибридные кандидаты и раскрытый контекст только для аудита. `CHAT_RETRIEVAL_EVIDENCE_ENABLED` по умолчанию выключен; включать его можно лишь после ручной оценки shadow-выборки. Даже при включении в prompt попадают только кандидаты не ниже `CHAT_RETRIEVAL_EVIDENCE_MIN_SCORE`.

Проверенный вариант без отдельного API key — hosted Exa MCP через `mcp-remote`:

```env
SEARCH_ENABLED=true
SEARCH_MCP_COMMAND=npx
SEARCH_MCP_ARGS="-y mcp-remote https://mcp.exa.ai/mcp"
SEARCH_MCP_ENV=PATH,HOME
SEARCH_MCP_TIMEOUT_SEC=30
SEARCH_QUERY_TIMEOUT_SEC=20
SEARCH_MCP_TOOL_WEB=web_search_exa
SEARCH_MCP_TOOL_GITHUB=web_search_exa
SEARCH_MCP_TOOL_REDDIT=web_search_exa
SEARCH_MCP_TOOL_FETCH=web_fetch_exa
SEARCH_FETCH_TOP_N=4
SEARCH_FETCH_MAX_CHARS=16000
```

Для новостей об утилитах можно добавить GitHub MCP поверх Exa, чтобы `source=github` ходил в GitHub issues/code отдельно:

```env
GITHUB_PERSONAL_ACCESS_TOKEN=
SEARCH_GITHUB_MCP_COMMAND=npx
SEARCH_GITHUB_MCP_ARGS="-y @modelcontextprotocol/server-github"
SEARCH_GITHUB_MCP_ENV=PATH,HOME,GITHUB_PERSONAL_ACCESS_TOKEN
SEARCH_GITHUB_MCP_TOOLS=search_issues,search_code
```

`PATH,HOME` нужны не Exa, а `npx`/`mcp-remote` после `env_clear()`. Значения не логируются.

Voice transcription:

```env
VOICE_TRANSCRIPTION_ENABLED=false
VOICE_AUTO_TRANSCRIBE=false
VOICE_MAX_DURATION_SEC=600
VOICE_MAX_FILE_MB=20
VOICE_SHORT_TEXT_MAX_CHARS=400
VOICE_LANGUAGE=ru
VOICE_ASR_PROVIDER=groq
VOICE_ASR_MODEL=whisper-large-v3
VOICE_ASR_TEMPERATURE=0
VOICE_CLEANUP_PROVIDER=
VOICE_CLEANUP_MODEL=
VOICE_CLEANUP_TEMPERATURE=0.2
VOICE_CLEANUP_MAX_TOKENS=1800
VOICE_RENDER_EXPANDABLE_CHAPTERS=true
VOICE_SEND_FULL_FILE=true
```

Для изображений в постах первого комментария используется отдельный лимит:

```env
FIRST_COMMENT_MAX_IMAGE_MB=10
```

Если Telegram сообщает размер файла выше лимита, бот не скачивает изображение и продолжает генерацию текстового комментария.

Правила voice-конфига:

- `VOICE_TRANSCRIPTION_ENABLED=false` полностью выключает voice pipeline, включая `/transcribe`.
- `VOICE_AUTO_TRANSCRIBE=false` выключает обработку обычных сообщений, но оставляет доступной ручную `/transcribe` reply-команду.
- `VOICE_ASR_PROVIDER=groq` - сейчас единственный поддержанный ASR provider.
- `VOICE_ASR_MODEL=whisper-large-v3` - дефолт для точной мультиязычной расшифровки в пределах Free Plan лимитов Groq.
- `VOICE_CLEANUP_PROVIDER` пустой значит использовать обычный `LLM_PROVIDER`.
- `VOICE_CLEANUP_MODEL` пустой значит использовать модель обычного provider-а.
- `VOICE_SHORT_TEXT_MAX_CHARS=400` значит короткая расшифровка после cleanup отправляется как простой текст без глав и времени.
- `VOICE_MAX_FILE_MB=20` выбран под cloud Bot API `getFile`; для больших файлов нужен local Bot API server.
- Если обычный HTML не влезает в безопасный лимит Telegram, бот отправляет Rich Message с закрытым блоком полного текста. `VOICE_SEND_FULL_FILE=true` оставляет `preview + voice-transcript.txt` только как fallback при ошибке Rich API или превышении rich-лимита.

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

Текущий deploy `v0.11.0` сделан на `vps-153`. Release binary соответствует merge commit [`bf59fd9`](https://github.com/Mar2ianen/NedoBot/commit/bf59fd9); immutable tag [`deploy-2026-07-31-v0.11.0`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-07-31-v0.11.0) фиксирует этот boundary. Предыдущий voice flow release отмечен [`deploy-2026-07-31-voice-flow`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-07-31-voice-flow), unified audit — [`deploy-2026-07-30-unified-audit`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-07-30-unified-audit). Последующие commits разрабатываются в `dev` и не считаются deployed до отдельного merge/review.


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

- Миграция `20260717180000_mcp_public_views.sql` задаёт scope и explicit-колонки. Private chat/DM и raw Telegram JSON не публикуются; полный reviewed inventory опубликованных view и полей находится в [`MCP_PUBLIC_DATA.md`](MCP_PUBLIC_DATA.md).
- `config/mcp_db_manifest.toml` — проверяемый allowlist views, колонок и их типов, а [`MCP_PUBLIC_DATA.md`](MCP_PUBLIC_DATA.md) — его human-readable snapshot. При старте MCP сверяет manifest с БД и отказывается стартовать при schema drift.
- Внешнему клиенту доступны только структурированные `db.*` и read-only domain tools; значения передаются bind-параметрами, лимит одной страницы — 200, effective column list — 40. Generic page собирается до logical rows budget 480 KiB, затем возвращает корректные `has_more`/`next_cursor`; запас учитывает дублирование RMCP text и structured content в wire response. Широкие views требуют явно передать `columns`. Одно text-поле может содержать до 8192 символов, domain message tools возвращают preview до 4096 символов; при превышении text, JSONB и array поля сообщают `_truncated_fields`, а preview заканчивается `…`. Aggregate `min`/`max` возвращает полное значение либо контролируемую ошибку budget. Соединений с БД — два, `statement_timeout` — 5 секунд.
- `db.search_text` и `chat.search_messages` принимают `match_mode`: `contains` (дефолтный поиск подстроки) или `whole_word` (точное слово/фраза с PostgreSQL word boundaries). Флаг `case_sensitive=false` по умолчанию; для имён и терминов без ложных совпадений вроде `Оля`/`доля` использовать `match_mode: "whole_word"`.
- JSON рекурсивно очищается от ключей наподобие `token`, `secret`, `authorization`, `database_url` и `invite_link`. В логах сохраняются только tool, table/columns/operators, количество строк и latency — без текстов сообщений и значений фильтров.

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

Ручной redeploy из локальной папки:

```bash
rsync -az --delete --exclude target --exclude .git --exclude .env ./ vps-153:/opt/tg-ai-bot-teloxide/
ssh vps-153 'cd /opt/tg-ai-bot-teloxide && /root/.cargo/bin/cargo build --release && systemctl restart tg-ai-bot-teloxide && systemctl is-active tg-ai-bot-teloxide && systemctl restart nedonews-mcp && systemctl is-active nedonews-mcp'
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
`OWNER_TELEGRAM_ID`; первое решение атомарно закрывает запрос и убирает
клавиатуру. Технические labels риска в карточке переводятся в понятные причины.

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
- рейтинг считается как `similarity * temporal_coefficient`, где коэффициент свежести плавно снижается от `1.0` к `0.70`, а период полураспада настраивается через `RAG_TEMPORAL_HALF_LIFE_DAYS`;
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

1. Проверить `VOICE_TRANSCRIPTION_ENABLED` и `VOICE_AUTO_TRANSCRIBE` для автоматического режима; `/transcribe` требует только `VOICE_TRANSCRIPTION_ENABLED`.
2. Отфильтровать чужие чаты, ботов, команды и automatic forwards.
3. Определить `VoiceMedia` из `voice`, `audio` или `video_note`.
4. Сохранить исходное Telegram message в `telegram_messages`.
5. Создать `voice_transcription_jobs`; повтор того же `(chat_id, message_id)` не создаёт новый job.
6. Проверить duration/file size до скачивания.
7. Скачать файл через Telegram `getFile` во временный файл.
8. Для `video_note` задать multipart MIME `video/mp4` и отправить исходный MP4 в Groq `/audio/transcriptions`.
9. Сразу после preflight отправить reply `Расшифровка…`; для обычного результата заменить его через `editMessageText`, а для Rich/file варианта обновить его статусом и отправить полный payload отдельным сообщением.
10. Сохранить raw ASR text, segments и raw JSON.
10. Запустить LLM cleanup по `prompts/voice_cleanup.md`.
11. Нормализовать clean result: короткий текст остаётся short, пустые/битые главы отбрасываются.
12. Собрать Telegram HTML через `telegram::html`.
13. Отправить reply: одно сообщение или preview + `voice-transcript.txt`.
14. Сохранить cleaned text, chapters JSON, final HTML и file id.

ASR request:

```text
POST https://api.groq.com/openai/v1/audio/transcriptions
model = VOICE_ASR_MODEL
response_format = verbose_json
language = VOICE_LANGUAGE
temperature = VOICE_ASR_TEMPERATURE
timestamp_granularities[] = segment
```

Cleanup request:

- сначала используется `VOICE_CLEANUP_PROVIDER`/`VOICE_CLEANUP_MODEL`, если заданы;
- если cleanup provider отличается от основного `LLM_PROVIDER` и падает, код пробует основной provider;
- если все cleanup providers падают, используется raw ASR transcript;
- если JSON от модели не парсится или cleanup меняет объём/числа сверх безопасных границ, используется raw ASR transcript.

Rendering policy:

- `clean.text.chars().count() <= VOICE_SHORT_TEXT_MAX_CHARS` -> только исправленный текст;
- `mode=chapters` + непустые chapters -> заголовок `Расшифровка голосового` и главы;
- тело главы идёт в `<blockquote expandable>`, если `VOICE_RENDER_EXPANDABLE_CHAPTERS=true` и обычное сообщение влезает;
- если HTML длиннее `SAFE_TEXT_LIMIT=3900`, бот отправляет Rich Message с закрытым `<details>`; rich-формат поддерживает до 32 768 символов;
- если Rich API отклоняет сообщение или rich-лимит превышен, `VOICE_SEND_FULL_FILE=true` включает fallback `preview + voice-transcript.txt`.

Текущий важный нюанс: `TranscriptChapter.start_sec` уже хранится, но `render.rs` пока не выводит timestamp рядом с заголовком главы. Это ближайший фикс в [REFACTOR_NEXT.md](REFACTOR_NEXT.md).

`video_note` Telegram не сопровождает MIME-типом, поэтому pipeline задаёт `video/mp4` сам. Groq принимает MP4 напрямую: отдельный `ffmpeg` и постоянное хранение кружков не нужны. Временный файл удаляется сразу после завершения ASR-запроса.

Cleanup prompt находится в `prompts/voice_cleanup.md`. Он должен чистить ASR, а не пересказывать голосовое: сохранять спорные формулировки автора, не менять числа/версии/названия моделей и учитывать локальный контекст канала `НедоNews`. В частности, `Gemma 4 31B` / `gemma4:31b` — валидная модель проекта, её нельзя заменять на `Gemma 2`, `Gemini` или `27B`.

## New User Audit

`src/features/new_user_analysis.rs` собирает профильные и поведенческие метрики новых/низкоактивных пользователей. Live flow запускает аудит после refresh профиля автора сообщения; `message_count >= 5` считается old-active baseline: snapshot сохраняется, но риск-сигналы не начисляются.

`NEW_USER_AUDIT_ENABLED=false` по умолчанию. При `NEW_USER_AUDIT_ENABLED=true` и `NEW_USER_AUDIT_AUTHORITATIVE_ENABLED=false` unified worker выполняет shadow-анализ и сохраняет assessment без изменения authoritative score/review. Для cutover включите оба флага: authoritative flow атомарно сохраняет baseline и job, затем materialize-ит итоговый score/review. Startup validation требует выключить `AVATAR_CLASSIFIER_ENABLED` и `FIRST_MESSAGE_SPAM_ENABLED`, а также проверяет embedding-конфиг, поэтому параллельные источники риска и неполная materialization-конфигурация не попадут в production.

Для ручного пересчёта истории:

```bash
cargo run --release --bin analyze_new_users -- --limit 4000 --max-messages 1000000 --include-analyzed
```

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

```env
COMMENT_CUSTOM_EMOJI_ID=5445092965875729965
TECH_CUSTOM_EMOJI_ID=
AMD_CUSTOM_EMOJI_ID=5442995600201106682
RADEON_CUSTOM_EMOJI_ID=5442853853395436819
RYZEN_CUSTOM_EMOJI_ID=5444875271163364561
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
- Админки пока нет; настройки меняются через `.env` и рестарт сервиса.
