# AGENTS.md

Инструкции для AI-ассистентов, работающих с проектом NedoBot.

## Проект

Rust-бот для Telegram-чата `НедоNews Chat`. Первый комментарий под постом канала, память контекста, статистика, расшифровка голосовых.

- **Стек**: Rust 2024 edition, teloxide 0.13, sqlx 0.8 (PostgreSQL), reqwest 0.12
- **LLM-провайдеры**: Gemini (основной), Groq, Cerebras, OpenRouter, Ollama, OpenAI-compatible
- **ASR**: Groq Whisper
- **Бот**: `@nedostraj_bot`, чат `-1001932061163`, канал `-1001575496091`

## Структура

```
src/main.rs                          — dispatcher, handler wiring, startup checks
src/config.rs                        — Config из .env, validate_runtime_secrets
src/state.rs                         — AppState(pool, config)
src/http.rs                          — кэшированные reqwest::Client с proxy
src/text.rs                          — normalize_ai_markers, strip_links, first_text_chars
src/db/mod.rs                        — build_pool, migrate
src/db/telegram.rs                    — CRUD: сообщения, пользователи, реакции, member events
src/telegram/commands.rs             — enum Command (BotCommands derive)
src/telegram/command_handler.rs      — dispatch команд, /userstats reply detection
src/telegram/render.rs               — send_html, send_rich_html (Bot API 10.1), escape_html
src/telegram/html.rs                 — Html builder (text/bold/code/link/custom_emoji/expandable_blockquote)
src/telegram/entities.rs             — forwarded_channel_post, message_text, custom_emoji_ids, message_has_links
src/telegram/custom_emoji.rs         — /emojiids diagnostic command
src/llm/mod.rs                       — модуль LLM
src/llm/types.rs                     — LlmRequest, LlmResponse, GeneratedText, LlmClient trait
src/llm/service.rs                   — generate_text с fallback-цепочкой, output validator
src/llm/gemini.rs                    — Gemini API (thinking budget, inline image)
src/llm/ollama.rs                    — Ollama /api/chat
src/llm/openai_compat.rs             — Groq, Cerebras, OpenRouter, custom OpenAI-compatible
src/features/first_comment/mod.rs
src/features/first_comment/pipeline.rs — maybe_comment_post: detect → clean → prompt → LLM → render → send
src/features/first_comment/candidate.rs — comment_candidate: только auto-forward из source_channel_id
src/features/first_comment/clean.rs    — should_generate_comment, clean_post_for_llm (отрезает signature)
src/features/first_comment/prompt.rs   — build_llm_prompt: system + tech_rag + memory + recent_comments + post
src/features/first_comment/quality.rs  — validate_comment_output: длина, CHAT_LINK, CTA, кириллица, generic phrases
src/features/first_comment/render.rs   — build_comment_html: strip_links → escape → CHAT_LINK → custom_emoji
src/features/first_comment/repo.rs     — post_comment_jobs, llm_generations CRUD
src/features/memory/service.rs         — atomic post history jobs, RAG retrieval, bounded retry and lease-safe finalization
src/features/memory/report.rs          — /memory command
src/features/stats/types.rs            — StatsPeriod (Day/Week/Month), StatsRender (Html/Rich), UserPresentation
src/features/stats/report.rs           — /stats_day, /stats_week, /stats_month, /topmsg, /topreact, /userstats, /userstatus
src/features/voice/pipeline.rs         — maybe_transcribe_voice → download → ASR → cleanup → render → send
src/features/voice/download.rs          — validate_media (duration/filesize), download_voice_file (tempfile)
src/features/voice/asr.rs               — Groq /audio/transcriptions multipart
src/features/voice/cleanup.rs           — LLM cleanup: prompt → generate → parse JSON → normalize_terms
src/features/voice/render.rs            — plain text / chapters / preview+file
src/features/voice/types.rs             — VoiceMedia, AsrTranscript, CleanTranscript, TranscriptChapter
src/features/voice/repo.rs              — voice_transcription_jobs CRUD
src/features/user_profiles/service.rs   — refresh_profile: get_chat, get_user_profile_photos, getUserPersonalChatMessages
src/features/new_user_analysis.rs       — new user audit: features, risk scoring, spam classification
src/bin/import_telegram_export.rs       — CLI: импорт Telegram Desktop export
src/bin/refresh_chat_members.rs         — CLI: refresh member snapshots
src/bin/refresh_user_profiles.rs        — CLI: batch profile refresh
src/bin/retry_pending_comments.rs      — CLI: retry failed comment jobs
src/bin/analyze_new_users.rs            — CLI: batch new user analysis
prompts/first_comment.md                — system prompt для первого комментария
prompts/tech_rag.md                     — ручной техно-RAG (release notes, version facts)
prompts/voice_cleanup.md                — system prompt для cleanup ASR transcript
docs/TECHNICAL.md                       — публичная документация проекта
docs/REFACTOR_NEXT.md                   — активный инженерный план
docs/REFACTOR_DONE.md                   — архив завершённого рефакторинга
migrations/                            — sqlx compile-time миграции
```

## Команды бота

| Команда | Описание |
|---------|----------|
| `/help` | Меню команд |
| `/ping` | Проверка живости |
| `/db` | Проверка подключения к БД |
| `/emojiids` | Показать custom_emoji_id из сообщения |
| `/format_test <text>` | Тест рендера первого комментария |
| `/memory` | Последние заметки памяти |
| `/stats_day [-r\|-p]` | Статистика дня (05:00 МСК) |
| `/stats_week [-r\|-p]` | Статистика недели |
| `/stats_month [-r\|-p]` | Статистика месяца |
| `/status day\|week\|month [-r\|-p]` | Alias для статистики |
| `/topmsg [-r\|-p]` | Топ 20 по сообщениям |
| `/topreact [-r\|-p]` | Топ 20 по реакциям |
| `/userstats <id\|username> [-r\|-p]` | Карточка пользователя |
| `/userstatus <id\|username> [-r\|-p]` | Alias /userstats |

`-r` = rich HTML (дефолт), `-p` = plain text. Reply на сообщение работает как implicit target для `/userstats`.

## Конфигурация

Все через `.env`. Шаблон: `.env.example`. Валидация секретов на старте в `Config::validate_runtime_secrets`.

**LLM routing**: `LLM_PROVIDER` определяет основной провайдер. Модель через `LLM_MODEL` или provider-specific переменную (`GROQ_MODEL`, `CEREBRAS_MODEL`, и т.д.). Для Gemini: fallback chain → flash-lite → ollama. Для других провайдеров — без fallback (fail-fast на старте, если модель не задана).

**Thinking budget**: Gemini `GEMINI_THINKING_BUDGET` добавляется к `LLM_MAX_TOKENS` в `maxOutputTokens` (thinking + answer). Output validator отдельно контролирует длину финального комментария.

**Proxy**: `LLM_PROXY_URL` — SOCKS5/HTTP proxy только для LLM запросов, не для Telegram polling.

## Критические правила для правок

### SQL — безопасность
- **Все** запросы через `sqlx::query_as()` / `sqlx::query()` с позиционными биндингами `$1, $2, ...`.
- Единственное исключение: `StatsPeriod::start_sql()` возвращает `&'static str` и встраивается через `format!` — это безопасно, но pattern нужно сохранять (только static str).
- `new_user_analysis.rs` использует `QueryBuilder` с динамическими именами колонок из `&'static [&'static str]` — безопасно, но хрупко. Не заменять на строки из переменных.
- **Никогда не подставлять пользовательский ввод (username, text, id) через `format!` в SQL.**

### HTML — экранирование
- **Всё**, что отправляется как `ParseMode::Html`, должно пройти через `telegram::html::Html::text()`, `Html::bold()`, `Html::link()` и т.д.
- `Html::raw_trusted()` использовать только для внутренне сконструированного HTML (уже экранированного). **Никогда не использовать для LLM output или пользовательского текста.**
- `escape_html()` = `html::escape()` = замена `&`, `<`, `>`, `"`.

### LLM output — санитизация
- `strip_links()` — удаляет ссылки, обёрнутые пунктуацией.
- `normalize_ai_markers()` — заменяет long dash/quotes и некоторые AI-маркеры.
- `validate_comment_output()` — reject если нет `{CHAT_LINK}`, дубль, raw URL, generic CTA, мало кириллицы, слишком длинный/короткий.
- `render_chat_link_placeholder()` — whitelist из 8 label-ов. Unknown label → текстом (не ссылка). URL жёстко из `config.chat_invite_url`.

### Secrets — не утекать
- API ключи передаются через `bearer_auth()` или `header("x-goog-api-key", ...)`.
- **Не логировать** response bodies от API при ошибках. `send_rich_message_request` извлекает body только для разбора безопасных `error_code`/`description`; raw body и URL с токеном не должны попадать в ошибки.
- Токен в URL для raw API calls (`getUserPersonalChatMessages`, `sendRichMessage`) — ок, но не логировать URL при ошибках.

### Telegram Bot API 10.1
- `sendRichMessage` / `InputRichMessage` — реализовано, но `#[allow(dead_code)]`, не используется в production.
- `getUserPersonalChatMessages` — используется в profile refresh (Bot API 10.1, June 11, 2026).
- `chatFullInfo` поля (`emoji_status_custom_emoji_id`, `profile_accent_color_id`) — используются.

## Потоки данных

### Первый комментарий
1. Telegram auto-forward из канала → `handle_message` → `spawn_message_author_profile_refresh`
2. `maybe_comment_post`: check `discussion_chat_id` + `source_channel_id` → check `post_signature_marker` → create job (dedup)
3. Download largest photo → base64
4. `build_llm_prompt`: system prompt + tech_rag + memory notes + recent comments + post text
5. `generate_text_checked`: provider → model → fallback chain → output validator (`validate_comment_output`)
6. `build_comment_html`: strip_links → normalize_ai_markers → escape → CHAT_LINK replacement → custom_emoji
7. `send_html_reply` → `mark_post_comment_sent` → `insert_llm_generation` → owner preview
8. `enqueue_post_history`: отдельная job → LLM JSON summary → RuBERT embedding → `post_history_entries`; retry с геометрическим backoff до terminal `failed`

### Голосовые
1. `maybe_transcribe_voice`: check enabled + auto + right chat + not bot + not command + not auto-forward
2. `VoiceMedia::from_message` → `create_voice_job` (dedup) → `validate_media` (duration/filesize/video_note)
3. Download → tempfile (auto-delete via TempPath)
4. ASR: Groq multipart → parse response
5. Cleanup: LLM → parse JSON/Plain → `normalize_terms` (groq, Gemma, etc.)
6. Render: short text / chapters / preview + file
7. Send → save result

### Статистика
- `/stats_day|week|month` / `/status`: `StatsPeriod::start_sql()` → aggregate queries → HTML/Rich report
- `/topmsg`: top users by messages, exclude 777000/bots/channel posts
- `/topreact`: top messages by reaction counts with links
- `/userstats`: resolve target (numeric id → username lookup) → profile + chat_user + totals → rich HTML

## Тесты

```bash
cargo test
```

Test fixtures: каждая тестовая модуль определяет `fn config() -> Config`. При добавлении поля в `Config` — обновить все test configs (антипаттерн AP2, планируется общий helper).

## Промпты

Промпты вшиты через `include_str!`, поэтому после правки нужен rebuild.

- `prompts/first_comment.md` — persona, стиль, правила, anti-repeat, примеры. Модель должна вернуть plain text с одним `{CHAT_LINK}` placeholder.
- `prompts/tech_rag.md` — ручной факт-чек: релизы, версии, platform status.
- `prompts/voice_cleanup.md` — cleanup ASR: словарь сленга, правила нормализации, формат JSON output.

## Деплой

VPS `vps-153`, systemd service `tg-ai-bot-teloxide`, Postgres в Podman `tg-ai-bot-postgres`. См. `docs/LOCAL_WORKFLOW.md` для команд.

## Стиль кода

### Контроль потока
- Предпочитать `?` для early return вместо вложенных `if let` / `match`. Глубина вложенности ≤ 3.
- `if let Some(x) = expr { ... } else { return }` лучше чем `if let Some(x) = expr { if condition { ... } }`.
- Использовать `is_some_and()`, `is_none_or()` для кратких проверок. Избегать `map(|x| x.is_something()).unwrap_or(false)`.
- Длинные цепочки `.map().filter().collect()` — разделять на осмысленные `let` bindings с именами.
- `match` на enum-вариантах с Guard-ами (`if condition`) — разбивать на отдельные функции, если ветки > 10 строк.

### Хардкод и магические числа
- Chat ID `777000` (Telegram service user) — OK, один раз в `stats/report.rs`. Не размазывать.
- `SAFE_TEXT_LIMIT = 3900`, `TELEGRAM_TEXT_LIMIT = 4096` — константы в `telegram/html.rs`.
- Timeout-ы (`45s`, `60s`, `120s`) — вынести в `const` или config, если повторяются.
- Хардкод статических URL (`https://api.groq.com/openai/v1/audio/transcriptions`) — OK в пределах одного модуля, не дублировать.

### Функции
- Одна функция — одна обязанность. Если функция > 60 строк, подумать о разделении.
- `pipeline.rs` — координация (call A, then B, then C). Логику каждого шага — в отдельном модуле.
- Не выносить бизнес-логику в `main.rs`, `command_handler.rs` или `render.rs`. Они — wiring, не вычисления.
- Публичные функции с `#[allow(dead_code)]` — OK для заделов, но комментировать зачем и когда планируется использование.

### Тесты
- Test fixtures `fn config()` дублируются. Не плодить новые — добавить в существующий модуль или вынести в общий helper (планируется).
- Тесты называются описательно: `gemini_comments_fallback_to_flash_lite_then_gemma_31b`, не `test_1`.

### Ошибки
- `tracing::error!(%err, ...)` — OK. Не логировать `%err` если err может содержать секреты или большие тела ответов.
- `anyhow::bail!("descriptive message")` — всегда с контекстом, не просто `"failed"`.
- External API errors: логировать провайдер + модель + статус, но не тело ответа и не API ключ.

### Запрещённые антипаттерны из аудита 2026-07-26

#### Провайдеры и внешние API
- **Не добавлять брендовые LLM-клиенты и ветки router-а** для OpenAI-compatible API: Groq, Cerebras, OpenRouter, Alibaba Model Studio/Qwen и аналогичные endpoint-ы должны быть profiles одного транспорта.
- Для OpenAI-compatible Chat Completions использовать уже подключённый crate `async-openai` с `OpenAIConfig::with_api_base()` и `with_api_key()`. **Не писать новый клиент на `reqwest`** и не копировать `OpenAiCompatClient`.
- Отдельный native client допустим только для действительно другого протокола (`Gemini GenerateContent`, `Ollama /api/chat`, ASR multipart и т. п.).
- URL endpoint-а, имя env-переменной ключа, default model и capabilities должны быть в provider profile/config, а не в `match provider` или в коде. Секреты не хранить в TOML/profile и не логировать.
- Возможности модели (`supports_images`, structured-output mode, timeout) задавать конфигурацией profile; не выводить их из названия модели эвристиками для новых провайдеров.

#### MCP и публичные данные
- **Не реализовывать вручную JSON-RPC/MCP transport, protocol structs и lifecycle** для новых MCP путей. Использовать `rmcp`; stdio и Streamable HTTP должны быть transport adapters над общим read-model слоем.
- `rmcp` tools не получают произвольный SQL. Public и agent-only пути обязаны использовать разные allowlisted DTO/projections и явный scope.
- Public MCP view с сообщениями должен фильтровать по `chat_id = discussion_chat_id`; одного `source_channel_id` недостаточно. Нельзя публиковать private/foreign-chat rows, raw Telegram JSON, file IDs, invite links и секреты.
- Для public endpoint обязательны мягкие anti-spam limits (`limit_req`/`limit_conn` в Nginx либо эквивалентный rate limiter), body/timeout limits и безопасное логирование без request body.

#### Конфигурация и feature flags
- **Запрещён silent fallback при невалидной заданной env-переменной.** Если переменная есть, но не парсится, startup должен завершиться с понятной ошибкой; default разрешён только для отсутствующей переменной.
- Новая feature обязана иметь явный enable flag, собственные provider/model/secret зависимости и startup validation. Нельзя скрыто использовать ключ, модель или endpoint другой feature.
- Disabled feature не должна создавать фоновые jobs, постоянный backlog или выполнять внешние запросы. Перед enqueue и worker startup проверять один и тот же feature gate.
- Зависимые flags валидируются явно: например, evidence требует retrieval, retrieval требует корректного embedding config.

#### Фоновые задачи и retries
- **Не создавать неограниченное число `tokio::spawn`, ожидающих semaphore.** Для массовых событий использовать bounded `mpsc` queue и фиксированное число workers; при переполнении — controlled skip/debounce и метрика.
- Новая долговременная job обязана иметь deduplication, статус, claim с `FOR UPDATE SKIP LOCKED`, lease, bounded retry/backoff, terminal failure и безопасный error kind.
- Нельзя оставлять job в `pending` после live failure и полагаться только на ручной bin для восстановления. Первый комментарий должен следовать той же lifecycle policy.
- Не копировать новые retry/lease расписания по модулям: использовать общий policy/helper или документированное обоснование отличий.

#### Архитектура, SQL и хардкоды
- Renderer не выполняет SQL и не считает бизнес-метрики. HTML и Rich варианты одного отчёта получают общие typed DTO из repo/service слоя; **не дублировать queries между format paths**.
- `main.rs` и `command_handler.rs` — wiring/authorization/transport. Не добавлять туда orchestration из нескольких domain actions; выносить в service/pipeline.
- Не размазывать service IDs, poll intervals, lease durations, retry delays и spam thresholds. Повторяющиеся значения — именованные constants/policy. `777000` должен иметь единственный source of truth.
- Исключение: public MCP scope IDs в reviewed migrations/manifest и protocol limits могут быть статическими, но не должны дублироваться в runtime без необходимости.
- Не добавлять module-wide `#![allow(dead_code)]` и blanket `#[allow(clippy::...)]`. Временное точечное исключение допустимо только с комментарием о причине и условии удаления.

#### Проверки
- Для нетривиальной правки обязательны `cargo fmt`, `cargo test --all-targets` и `cargo clippy --all-targets -- -D warnings`.
- Изменения public MCP projections, job lifecycle, config gates, risk transitions и shared stats data обязаны получать integration tests с PostgreSQL. Unit tests не заменяют проверку миграций и SQL scope.

## Дисциплина коммитов

### Формат
```
<тип>: <краткое описание на английском, imperative>
```

Типы:
- `feat` — новая фича
- `fix` — исправление бага
- `refactor` — рефакторинг без изменения поведения
- `docs` — документация
- `test` — тесты
- `chore` — deps, tooling, config

Примеры:
```
feat: add /status command as stats period alias
fix: validate_chat_link_token rejects labels with spaces
refactor: extract html builder into telegram::html module
docs: update TECHNICAL.md with voice pipeline details
```

### Правила
- Один коммит — одна логическая единица изменения. Не смешивать фичу с рефакторингом в одном коммите.
- Перед коммитом: `cargo fmt && cargo test`.
- Если меняем промпт — отдельный коммит с описанием что и почему.
- Если меняем SQL-миграцию — отдельный коммит. Проверить backward compatibility.
- Если меняем Config (новые поля) — обновить struct + from_env + все test fixtures + .env.example.
- Commit message тело (если нужно): описать контекст, мотивацию, что пробовалось. Не dump diff.

## Bot API 10.1 — локальные изменения

Бот использует три метода из **Bot API 10.1** (June 11, 2026). Все через raw HTTP, teloxide их не оборачивает.

### getUserPersonalChatMessages
**Где:** `src/features/user_profiles/service.rs:253-281`

```
POST https://api.telegram.org/bot{token}/getUserPersonalChatMessages
Body: { "user_id": i64, "limit": 5 }
```

Возвращает последние сообщения из личного канала пользователя. Используется для:
- Детектирования adult-спама (promo DM bait, personal channel promotion)
- Анализа текста личного канала нового пользователя
- Ошибки: `USER_PERSONAL_CHANNEL_MISSING` — канал отсутствует, считается definitive

### sendRichMessage
**Где:** `src/telegram/render.rs:44-212` (`#[allow(dead_code)]` — не используется в production)

```
POST https://api.telegram.org/bot{token}/sendRichMessage
Body: { chat_id, rich_message: { html | markdown, is_rtl?, skip_entity_detection? }, reply_parameters?, ... }
```

Rich Messages — структурированный текст до 32 KB с вложениями, entity detection, RTL. Реализовано:
- `InputRichMessage::html()`, `InputRichMessage::markdown()`
- `send_rich_html()`, `send_rich_html_reply()`
- `normalize_rich_text()` с лимитом 32 768 символов

Задел для будущих rich-отчётов статистики.

### chatFullInfo поля
**Где:** `src/features/user_profiles/service.rs:185-187`

```rust
chat.chat_full_info.emoji_status_custom_emoji_id
chat.chat_full_info.profile_accent_color_id
```

teloxide оборачивает эти поля из `ChatFullInfo` (Bot API 10.1). Используются в профиле пользователя для:
- Custom emoji status пользователя
- Accent color профиля

### Типы teloxide, затронутые Bot API 10.1
- `Chat` получил `chat_full_info: ChatFullInfo`
- `UserProfilePhotos` — без изменений
- `MessageOrigin::Channel` — без изменений
- teloxide 0.13 обновлён для 10.1

## Правила для AI-ассистента

1. **Не коммитить `.env`**, токены, экспорты Telegram, дампы БД.
2. **Изменять промпты аккуратно** — они определяют поведение бота в проде. Правка промпта = redeploy.
3. **SQL-миграции**: новый файл в `migrations/` с timestamp prefix. После добавления — `touch src/db/mod.rs` для sqlx recompile.
4. **Новые Config-поля**: добавить в struct, `from_env()`, все test `fn config()`, `.env.example`, docs.
5. **Новые команды**: enum `Command` в `commands.rs` + handler в `command_handler.rs` + обновить README и TECHNICAL.
6. **Новые LLM-провайдеры**: реализовать `LlmClient` trait, добавить в `generate_once` match + `model_for_provider` + `validate_llm_provider_secret`.
7. **Не ломать backward compatibility**: бот работает в проде, старые записи в БД. Миграции только additive.
8. **Comment density**: код хорошо документирован. Писать комментарии по делу, не_water.
9. **Russian** — код, комментарии, промпты, документация на русском. Git commit messages на английском (imperative).
10. **Проверять перед правками**: `cargo test` после каждого non-trivial change. Формат: `cargo fmt`.
