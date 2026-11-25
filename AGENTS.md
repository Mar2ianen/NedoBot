# AGENTS.md

Инструкции для AI-ассистентов, работающих с проектом NedoBot. Правила ниже обязательны; примеры путей и обзор потоков — навигация, а не замена проверке актуального кода.

## Перед началом задачи

- Определи целевую ветку, точный HEAD, область изменений и состояние worktree. Не перезаписывай чужие изменения, не смешивай независимые PR и не считай состояние другой ветки состоянием текущей.
- Открой ближайшие нормативные документы по таблице. Не считай существующий код автоматически корректным образцом и не правь исторические документы как описание текущего состояния.
- Если документация и код расходятся, установи назначение документа (контракт, описание runtime, changelog или исторический аудит), затем определи дефект. Не меняй нормативный контракт только ради прохождения тестов.

| Область | Что прочитать до правки |
|---|---|
| Любой нетривиальный Rust-код | [CODE_STYLE](docs/CODE_STYLE.md), [ARCHITECTURE](docs/ARCHITECTURE.md), [TEST_QUALITY](docs/TEST_QUALITY.md) |
| Карточки, статистика, репорты, Rich/HTML/fallback | [CARD_STYLE](docs/CARD_STYLE.md), соответствующий renderer и delivery adapter |
| Конфигурация, features, provider routing | [TECHNICAL](docs/TECHNICAL.md), [COMMUNITY_CONFIG](docs/COMMUNITY_CONFIG.md), `Cargo.toml`, `config/llm_profiles.toml.example` |
| URL/QR/антиспам, public MCP, Ask memory | [TEST_QUALITY](docs/TEST_QUALITY.md), [TECHNICAL](docs/TECHNICAL.md), scope/policy соответствующего feature |
| Тесты и release | [DEVELOPMENT](docs/DEVELOPMENT.md), [TEST_QUALITY](docs/TEST_QUALITY.md), текущие CI и scripts |
| Изменения зависимостей | [SECURITY_DEPENDENCIES](docs/SECURITY_DEPENDENCIES.md), `Cargo.toml`, `Cargo.lock`, upstream advisory/API |

## Проект

Rust-бот для сообществ NedoNews и PVO. Один release binary использует раздельные runtime-конфигурации и PostgreSQL базы; функции включаются compile-time features и per-instance config.

- **Стек**: Rust 2024 edition, pinned fork teloxide (версия и rev — `Cargo.toml`/`Cargo.lock` текущей ветки), SQLx 0.9 (PostgreSQL; SQLite для spam reputation), reqwest 0.13, `genai`, `rmcp`. Не переносить wire/API предположения между RC без проверки.
- **LLM-провайдеры**: Gemini, Groq, Cerebras, OpenRouter, Ollama, OpenAI-compatible; primary/fallback определяются отдельно для каждого route, единого основного провайдера нет.
- **ASR**: Gemini и Groq; primary/shadow определяются runtime profile и media capabilities.
- **Бот**: `@nedostraj_bot`, чат `-1001932061163`, канал `-1001575496091`

## Структура

```
src/main.rs                          — dispatcher, handler wiring, startup checks
src/config.rs                        — Config из profile TOML и secrets env, validate_runtime_secrets
src/state.rs                         — AppState(pool, config)
src/http.rs                          — кэшированные reqwest::Client с proxy
src/lib.rs                            — crate exports; `text` re-exported from pinned `teloxide-antispam`
src/db/mod.rs                        — build_pool, migrate
src/db/telegram.rs                    — CRUD: сообщения, пользователи, реакции, member events
src/telegram/commands.rs             — enum Command (BotCommands derive)
src/telegram/command_handler.rs      — dispatch команд и Telegram-adapter для /ask
src/telegram/render.rs               — send_html, Rich Message helpers, escape_html
src/telegram/html.rs                 — Html builder (text/bold/code/link/custom_emoji/expandable_blockquote)
src/telegram/entities.rs             — forwarded_channel_post, message_text, custom_emoji_ids, message_has_links
src/telegram/custom_emoji.rs         — /emoji_ids diagnostic command
src/llm/mod.rs                       — модуль LLM
src/llm/types.rs                     — typed generation, attempt, transport error и output types
src/llm/service.rs                   — generation через выбранный route/profile и output validation
src/llm/profiles.rs                  — profile/route configuration
src/llm/genai_transport.rs           — provider transport adapter
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
src/features/stats/report.rs           — /stats_day, /stats_week, /stats_month, /topmsg, /bottommsg, /topreact, /userstats, /userstatus
src/features/voice/pipeline.rs         — maybe_transcribe_voice → download → ASR → cleanup → render → send
src/features/voice/download.rs          — validate_media (duration/filesize), download_voice_file (tempfile)
src/features/voice/asr.rs               — Gemini/Groq ASR, primary/shadow и media fallback
src/features/voice/cleanup.rs           — LLM cleanup: prompt → generate → parse JSON → normalize_terms
src/features/voice/render.rs            — plain text / chapters / preview+file
src/features/voice/types.rs             — VoiceMedia, AsrTranscript, CleanTranscript, TranscriptChapter
src/features/voice/repo.rs              — voice_transcription_jobs CRUD
src/features/user_profiles/service.rs   — refresh_profile: get_chat, get_user_profile_photos, get_user_personal_chat_messages
src/features/user_profiles/enrichment.rs — bounded queue: profile refresh → audit → spam/review/avatar jobs
src/features/new_user_analysis.rs       — unified new-user audit snapshot and baseline risk scoring
src/features/new_user_audit/             — unified profile, avatar and first-message assessment
src/features/spam_review.rs             — idempotent review-карточки для новых пользователей
src/features/reports/                   — пользовательские репорты, durable PM delivery и rich-карточки
src/features/ask/                       — /ask agent, audit и RMCP child client
src/features/chat_read_api/             — transport-agnostic public read-model и manifest catalog
src/mcp/                                — ChatMcpServer и RMCP stdio/Streamable HTTP adapters
src/bin/import_telegram_export.rs       — CLI: импорт Telegram Desktop export
src/bin/refresh_chat_members.rs         — CLI: refresh member snapshots
src/bin/refresh_user_profiles.rs        — CLI: batch profile refresh
src/bin/retry_pending_comments.rs      — CLI: retry failed comment jobs
src/bin/chat_db_mcp.rs                  — internal RMCP stdio server для /ask
src/bin/nedonews_mcp_http.rs             — public RMCP Streamable HTTP server
prompts/first_comment.md                — system prompt для первого комментария
prompts/tech_rag.md                     — ручной техно-RAG (release notes, version facts)
prompts/voice_cleanup.md                — system prompt для cleanup ASR transcript
docs/TECHNICAL.md                       — публичная документация проекта
docs/CARD_STYLE.md                      — обязательный стиль карточек, рейтингов и модерации
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
| `/emoji_ids` | Показать custom_emoji_id из сообщения |
| `/format_test <text>` | Тест рендера первого комментария |
| `/memory` | Последние заметки памяти |
| `/ask <вопрос>` | Агентный поиск по истории чата и публичному read-model |
| `/chat_note <текст>` | Добавить общую заметку чата |
| `/user_note <текст>` | Добавить заметку о пользователе reply |
| `/report [причина]` | Пожаловаться на человеческое сообщение reply; карточка уходит администраторам в личку |
| `/stats_day [-r\|-p] [-e]` | Статистика дня (05:00 МСК) |
| `/stats_week [-r\|-p] [-e]` | Статистика недели |
| `/stats_month [-r\|-p] [-e]` | Статистика месяца |
| `/status day\|week\|month [-r\|-p] [-e]` | Alias для статистики |
| `/topmsg [-r\|-p] [-e]` | Топ 20 по сообщениям |
| `/topword <слово> [-r\|-p] [-e]` | Топ по числу вхождений слова |
| `/topreact [-r\|-p] [-e]` | Топ 20 по реакциям |
| `/bottommsg [-r\|-p] [-e]` | 20 самых молчаливых |
| `/userstats <id\|username> [-r\|-p] [-e]` | Карточка пользователя |
| `/userstatus <id\|username> [-r\|-p] [-e]` | Alias /userstats |
| `/transcribe` | Расшифровать voice, audio или кружок в reply |
| `/forget` | Забыть личную память диалога |
| `/notspam` | Пометить сообщение как не спам (reviewer) |
| `/mute`, `/ban`, `/warn` | Санкции модерации |
| `/unmute`, `/unban`, `/unwarn` | Снять санкции |
| `/warns`, `/modlog`, `/undo` | История и отмена действий модерации |

`-r` = Rich (дефолт), `-p` = обычный Telegram HTML, `-e` = ephemeral ответ только автору команды в группе при разрешённой policy. Plain text — отдельный fallback доставки. Reply на сообщение работает как implicit target для `/userstats`.

## Конфигурация

Несекретные runtime-настройки хранятся в обязательной `[runtime]` секции существующего `config/llm_profiles.toml.example` (или в deployment-копии, выбранной через абсолютный `LLM_PROFILES_PATH`). Локальный `.env` не коммитится и содержит только секреты, DSN, invite/proxy URL и другие чувствительные значения; полный контракт приведён в `docs/TECHNICAL.md`. Отсутствующая `[runtime]` секция — startup error, defaults не подставляются. Валидация секретов выполняется на старте в `Config::validate_runtime_secrets`.

**LLM routing**: `LLM_PROFILES_PATH` необязателен и задаёт deployment-копию provider/model/task route topology; без него используется репозиторный example-файл. Каждый runtime-вызов передаёт явный route (`first_comment`, `memory`, `voice_cleanup`, `search_extract`, `new_user_audit` или `ask`); provider/model env overrides и hard-coded fallback chains удалены.

**Thinking budget**: для Gemini 3.x и новее бот использует `thinkingLevel` из профиля модели и не передаёт `temperature` или числовой thinking budget. Для старых Gemini-моделей `runtime.gemini_thinking_budget` добавляется к `runtime.llm_max_tokens` в `maxOutputTokens` (thinking + answer). Output validator отдельно контролирует длину финального комментария.

**Proxy**: `LLM_PROXY_URL` — SOCKS5/HTTP proxy только для LLM запросов, не для Telegram polling.

## Критические правила для правок

### Карточки и пользовательский вывод
- Перед изменением карточек, статистики, рейтингов или сообщений модерации **обязательно прочитать [docs/CARD_STYLE.md](docs/CARD_STYLE.md)** и выбрать соответствующий образец. Контракт обязателен для Rich, HTML и текстового fallback.
- Не добавлять дублирующие заголовки и декоративные подписи. `/topword` выводит только ранжированный список без заголовка, повторения слова, периода и шапки колонок.
- Сохранять кликабельные имена и проверенные источники, одинаковые данные между форматами, экранирование и безопасную доставку. Изменение стиля требует обновления контракта и примера «до / после» в том же PR.
- Один DTO/use case → разные presentation paths; общий field/identity/link contract важнее совпадения HTML-строк. Не брать Rich HTML как универсальный HTML: `<table>`, `<details>` и typed blocks не являются обычным Telegram ParseMode. Отдельно проверять реально выбранный fallback, а не только оба renderer при прямом вызове.
- Новое поле проходит цепочку repo/service → DTO → Rich → HTML → plain/file fallback по применимости. Проверить 0/unknown/absent, escaped Unicode, scope и links; при недоступном аватаре или профиле текст не пропадает.
- Для новых карточек — один предметный заголовок (или ни одного по образцу), до четырёх основных колонок без обоснования расширения, вторичные данные в details. Никаких декоративных footer/emoji ради унификации. Не менять смысл метрик или порядок при адаптации к экрану.
- У длинного результата заранее определить стратегию: целые строки/блоки, страницы или файл; считать длину штатным helper после экранирования. Не резать готовую разметку. При multi-send проверить частичную доставку: уже подтверждённые части не повторять, unknown не трактовать как rejection, audience/thread/reply сохраняются.
- В PR приложить безопасный пример **фактического** output каждого затронутого формата, по возможности rendered fixture. При изменении стиля — before/after и причина; при исправлении отклонения от существующего контракта не переписывать контракт под дефект.

### Разметка антиспама и Decision

- `record_spam()` **намеренно** помечает все сохранённые сообщения аккаунта,
  подтверждённого владельцем как спам-источник. Это защита от пакетных
  сообщений одноцелевых фейковых/userbot аккаунтов. Не удалять это поведение
  под видом исправления «загрязнения корпуса»; account-label остаётся
  исходным решением владельца.
- Для статистической оценки отличать произведённые этой операцией
  message stamps от независимой ручной annotation каждого сообщения.
- Проверять Telegram identity до пакетной классификации: `sender_chat`,
  `@Channel_Bot`, source_channel и сохранённый статус `kicked` не
  доказывают бан конкретного человека или причину модерации.
- Отдельный токен «приму» + сильный оффтоп относительно известной темы —
  кандидат для существующего Decision/captcha, **не прямой автобан**.
  Женское имя, аватар и ID-risk — дополнительные признаки, не обязательные
  условия; обычный оффтоп без «приму» этот путь не активирует.
- Для расследования пропусков Decision сначала различать «не queued»,
  «failed/stale», «NoSignal/Abstain», «ниже threshold» и «action gate».
  Сверять фактический instance runtime, не extrapolate example-конфиги.
  Контекст и проверки: `docs/DECISION_SHORT_BAIT_HANDOFF_2026-11-20.md`.

### SQL — безопасность
- Запросы через штатные `sqlx::query*` / `QueryBuilder` с позиционными bind-параметрами `$1, $2, ...`; не собирать SQL конкатенацией внешних значений.
- Dynamic SQL допускается только для проверенных статических фрагментов/идентификаторов из закрытого allowlist. `AssertSqlSafe` не является санитайзером: перед его использованием доказать происхождение каждого фрагмента; все значения передавать через bind. Для статистики использовать общий `ReportWindow`, population filter и aggregate helpers из `stats::repo`.
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
- **Не логировать** response bodies от внешних API при ошибках. Raw body и URL с секретами не должны попадать в ошибки.
- Telegram API вызывается через typed методы teloxide fork-а; application code не должен собирать token-bearing URLs или дублировать Telegram payload/response модели.

### Telegram Bot API — контракт pinned fork
- `Bot::send_rich_message` / `InputRichMessage` используются в production для rich stats, `/ask` и длинных расшифровок голосовых; при ошибке доступен безопасный HTML/file fallback.
- `Bot::get_user_personal_chat_messages` используется в profile refresh.
- `chatFullInfo` поля (`emoji_status_custom_emoji_id`, `profile_accent_color_id`) — используются.

## Потоки данных

### Первый комментарий
1. Telegram auto-forward из канала → `handle_message` → `spawn_message_author_profile_refresh`
2. `maybe_comment_post`: check `discussion_chat_id` + `source_channel_id` → check `post_signature_markers` (primary + retired aliases) → create job (dedup)
3. Download largest photo → base64
4. `build_llm_prompt`: system prompt + tech_rag + memory notes + recent comments + post text
5. `generate_text_checked`: task route → model fallback chain → output validator (`validate_comment_output`)
6. `build_comment_html`: strip_links → normalize_ai_markers → escape → CHAT_LINK replacement → custom_emoji
7. Durable claim/revision fence → `begin_post_comment_delivery` → typed Telegram effect → `finalize_post_comment_sent` либо подтверждённый rejection/`delivery_unknown`. При unknown не повторять send автоматически; сохранять audit и operator reconciliation.
8. `enqueue_post_history`: отдельная job → LLM JSON summary → RuBERT embedding → `post_history_entries`; retry с геометрическим backoff до terminal `failed`

### Голосовые
1. `maybe_transcribe_voice`: check enabled + auto + right chat + not bot + not command + not auto-forward
2. `VoiceMedia::from_message` → `create_voice_job` (dedup) → `validate_media` (duration/filesize/video_note)
3. Download → tempfile (auto-delete via TempPath)
4. ASR: `transcribe_audio_with_shadow` → primary transcript / optional comparative shadow; media fallback и provider errors по контракту ASR.
5. Cleanup: LLM → parse JSON/Plain → `normalize_terms` (groq, Gemma, etc.)
6. Render: short text / chapters / preview + file
7. Send → save result

### Статистика
- `/stats_day|week|month` / `/status`: `stats::repo::report_window` → единый `ReportWindow` для scoped aggregate queries → общий DTO → HTML/Rich report.
- `/topmsg`: top users by messages, exclude 777000/bots/channel posts
- `/topreact`: top messages by reaction counts with links
- `/userstats`: resolve target (numeric id → username lookup) → profile + chat_user + totals → rich HTML

## Тесты

```bash
cargo test
```

Для проверки workspace используйте команды из `docs/DEVELOPMENT.md`: default и `--no-default-features` test/check lanes запускаются отдельно. Успех сборки без feature не доказывает корректность runtime-конфигурации: fixtures должны отключать отсутствующие compile-time capabilities, а тесты feature-specific поведения — иметь соответствующий `#[cfg(feature = ...)]`.

Все зарегистрированные в `WorkerSupervisor` задачи являются long-lived workers. Неожиданное завершение worker должно завершать процесс ошибкой после bounded drain, чтобы systemd мог перезапустить сервис. Не превращайте panic/return такого worker в только запись в журнал.

Test fixtures: для unit-тестов crate переиспользовать `crate::config::test_config()` и точечно переопределять нужные поля. Для integration tests использовать shared fixture в `tests/support`, если она подходит; `#[cfg(test)]` helper библиотеки не доступен внешнему integration crate. Не копировать полный `Config`. Изменения Config должны сохранять feature-gated defaults и startup validation.

## Промпты

Промпты вшиты через `include_str!`, поэтому после правки нужен rebuild.

- `prompts/first_comment.md` — persona, стиль, правила, anti-repeat, примеры. Модель должна вернуть plain text с одним `{CHAT_LINK}` placeholder.
- `prompts/tech_rag.md` — ручной факт-чек: релизы, версии, platform status.
- `prompts/voice_cleanup.md` — cleanup ASR: словарь сленга, правила нормализации, формат JSON output.

## Деплой

VPS `vps-153`, Postgres в Podman `tg-ai-bot-postgres`. Два community-инстанса
одного release binary (default features): systemd `tg-ai-bot-teloxide`
(НедоNews, БД `tg_ai_bot`, профиль `/etc/tg-ai-bot/llm_profiles.toml`) и
`nedobot-pvo` (ПВО, БД `tg_ai_bot_pvo`, профиль
`/etc/tg-ai-bot/pvo-llm_profiles.toml`); общий SQLite спам-репутации
`/var/lib/nedobot/spam-reputation.sqlite` при разных `instance.id`.
Проверенные общие команды и ограничения деплоя описаны в `docs/TECHNICAL.md`;
Machine-specific local workflow notes не являются частью репозитория и не должны добавляться в commit.

### Живость обоих инстансов

- Проверять оба юнита, а не только `tg-ai-bot-teloxide`: `systemctl is-active
  tg-ai-bot-teloxide nedobot-pvo`, exe-хеш обоих PID против release binary,
  отсутствие `migration ... was previously applied but is missing` и `ERROR`
  в обоих journal за окно наблюдения.
- `inactive (dead)` любого из инстансов — инцидент: смотреть `journalctl -u
  <unit>`, чинить причину, не маскировать рестартом. Отсутствие апдейтов из
  Telegram дольше ~суток не наверстывается polling-ом (сервер отбрасывает
  неподтверждённые updates); историю дотягивать экспортом через
  `import_telegram_export`, а не задним числом.
- Deploy, restart и smoke всегда охватывают оба инстанса (см.
  `docs/DEPLOYMENT.md` и workflow `release`). Конфиги правятся вручную с
  бэкапом и автоматикой не перезаписываются.

### Ветки между deploy

- `main` — release line. Каждый фактический deploy отмечается immutable annotated tag `deploy-YYYY-MM-DD-<scope>` на точном commit release binary.
- Обычная post-deploy разработка идёт в `dev`, затем проходит отдельный review/merge из `dev` в `main`. Сначала проверь, что `dev` основан на актуальном `main`; если он отстаёт от релизной линии или задачу явно привязали к более свежему baseline, не создавай разветвившуюся историю и веди узкие PR от актуальной целевой ветки до явной синхронизации `dev`.
- Не считать `HEAD` автоматически задеплоенным: ориентироваться на deployment tag и запись в `docs/TECHNICAL.md`.
- Первый такой tag: `deploy-2026-07-30-unified-audit` указывает на `849b632`.

## Стиль кода

### Контроль потока
- Предпочитать `?` для early return вместо вложенных `if let` / `match`. Глубина вложенности ≤ 3.
- `if let Some(x) = expr { ... } else { return }` лучше чем `if let Some(x) = expr { if condition { ... } }`.
- Использовать `is_some_and()`, `is_none_or()` для кратких проверок. Избегать `map(|x| x.is_something()).unwrap_or(false)`.
- Длинные цепочки `.map().filter().collect()` — разделять на осмысленные `let` bindings с именами.
- `match` на enum-вариантах с Guard-ами (`if condition`) — разбивать на отдельные функции, если ветки > 10 строк.

### Хардкод и магические числа
- Telegram service user `777000` — использовать `stats::repo::TELEGRAM_SERVICE_USER_ID` там, где применим общий фильтр human activity; не дублировать литерал в новом коде.
- `SAFE_TEXT_LIMIT = 3900`, `TELEGRAM_TEXT_LIMIT = 4096` — константы в `telegram/html.rs`.
- Timeout-ы (`45s`, `60s`, `120s`) — вынести в `const` или config, если повторяются.
- Хардкод статических URL (`https://api.groq.com/openai/v1/audio/transcriptions`) — OK в пределах одного модуля, не дублировать.

### Функции
- Одна функция — одна ответственность. 60 строк — сигнал для review, не hard limit; выделять семантические этапы с проверяемыми контрактами, а не нарезать линейную композицию по LOC.
- `pipeline.rs` — координация (call A, then B, then C). Логику каждого шага — в отдельном модуле.
- Не выносить бизнес-логику в `main.rs`, `command_handler.rs` или `render.rs`. Они — wiring, не вычисления.
- Не добавлять мёртвый production-код «на будущее». Для необходимого временного `#[allow(dead_code)]`/`#[allow(unused_imports)]` — минимальная область, конкретная причина и условие удаления; не заглушать предупреждения ради зелёного CI.

### Тесты
- Переиспользовать общий `crate::config::test_config()` в unit-тестах; существующие дубликаты мигрировать при релевантных изменениях, без массовой косметической правки.
- Тесты называются описательно: `gemini_comments_fallback_to_flash_lite_then_gemma_31b`, не `test_1`.

### Ошибки
- `tracing::error!(%err, ...)` — OK. Не логировать `%err` если err может содержать секреты или большие тела ответов.
- `anyhow::bail!("descriptive message")` — всегда с контекстом, не просто `"failed"`.
- External API errors: логировать провайдер + модель + статус, но не тело ответа и не API ключ.

### Запрещённые антипаттерны из аудита 2026-07-26

#### Провайдеры и внешние API
- **Не добавлять брендовые LLM-клиенты и ветки router-а** для OpenAI-compatible API: Groq, Cerebras, OpenRouter, Alibaba Model Studio/Qwen и аналогичные endpoint-ы должны быть profiles одного транспорта.
- Для OpenAI-compatible Chat Completions использовать уже подключённый crate `genai` и существующую конфигурацию провайдера. **Не писать новый клиент на `reqwest`** и не копировать `OpenAiCompatClient`.
- Отдельный native client допустим только для действительно другого протокола (`Gemini GenerateContent`, `Ollama /api/chat`, ASR multipart и т. п.).
- URL endpoint-а, имя env-переменной ключа, default model и capabilities должны быть в provider profile/config, а не в `match provider` или в коде. Секреты не хранить в TOML/profile и не логировать.
- Возможности модели (`supports_images`, structured-output mode, timeout) задавать конфигурацией profile; не выводить их из названия модели эвристиками для новых провайдеров.

#### MCP и публичные данные
- **Не реализовывать вручную JSON-RPC/MCP transport, protocol structs и lifecycle** для новых MCP путей. Использовать `rmcp`; stdio и Streamable HTTP должны быть transport adapters над общим read-model слоем.
- `rmcp` tools не получают произвольный SQL. Internal stdio и public HTTP используют один намеренно публичный allowlisted read-model и явный scope; возможная отдельная agent memory должна оставаться самостоятельным backend/tool set и не публиковаться через HTTP автоматически.
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
- Перед коммитом Rust-изменений — проверки по `docs/DEVELOPMENT.md` и `docs/TEST_QUALITY.md`; для docs-only достаточно проверить ссылки, структуру и diff, если код/схемы/генераторы не менялись. Не представлять неподтверждённые checks как пройденные.
- Если меняем промпт — отдельный коммит с описанием что и почему.
- Если меняем SQL-миграцию — отдельный коммит. Проверить backward compatibility.
- Если меняем Config (новые поля) — обновить struct + from_env + все test fixtures + техническую документацию и tracked config template, если он добавлен в репозиторий.
- Commit message тело (если нужно): описать контекст, мотивацию, что пробовалось. Не dump diff.

## Bot API — typed fork methods

Pinned fork teloxide предоставляет typed методы для используемых расширений Bot API. Версии и доступные wire-типы проверяются по lockfile и исходникам соответствующей ревизии. Не добавлять для них raw HTTP-обёртки или самодельные wire-модели.

### `get_user_personal_chat_messages`
**Где:** `src/features/user_profiles/service.rs`

Возвращает последние сообщения из личного канала пользователя. Используется для:
- детектирования adult-спама (promo DM bait, personal channel promotion);
- анализа текста личного канала нового пользователя;
- обработки `USER_PERSONAL_CHANNEL_MISSING` как definitive-ошибки отсутствия канала.

### `send_rich_message`
**Где:** `src/telegram/render.rs`, `src/telegram/service_messages.rs`

Используется для rich-отчётов статистики, `/ask` и длинных расшифровок голосовых. Локальный renderer отвечает только за application-level лимит `32_768` символов, reply-параметры и безопасный fallback.

### `ChatFullInfo`
**Где:** `src/features/user_profiles/service.rs`

```rust
chat.emoji_status_custom_emoji_id
chat.profile_accent_color_id
```

Typed-модель teloxide предоставляет эти поля для сохранения custom emoji status и accent color профиля.

## Правила для AI-ассистента

1. **Не коммитить `.env`**, токены, экспорты Telegram, дампы БД.
2. **Изменять промпты аккуратно** — они определяют поведение бота в проде. Правка промпта = redeploy.
3. **SQL-миграции**: новый additive файл в `migrations/` с timestamp prefix. Проверить обнаружение миграции SQLx и clean/upgrade path на disposable PG; при необходимости инициировать rebuild.
4. **Новые Config-поля**: обновить typed config, parsing/validation, shared fixtures, документацию и tracked runtime/profile template; проверить disabled feature и неверные значения.
5. **Новые команды**: зарегистрировать `Command` в `commands.rs`, а transport adapter держать тонким и делегировать отдельному feature/use-case модулю. Не добавлять новую бизнес-логику в общий `command_handler.rs`; поддерживать общие справку/документацию. См. `docs/CODE_STYLE.md`.
6. **Новые LLM-провайдеры**: добавить provider/model profile и route selection в `config/llm_profiles.toml.example`; не добавлять provider match в service code.
7. **Не ломать backward compatibility**: бот работает в проде, старые записи в БД. Миграции только additive.
8. **Comment density**: код хорошо документирован. Писать комментарии по делу, не_water.
9. **Язык** — identifiers, API и типы сохраняют принятую английскую терминологию Rust; пользовательский вывод по `StatsStrings`/locale, русская документация и промпты по существующему стилю файла. Git commit messages на английском (imperative).
10. **Проверять перед правками**: для нетривиального изменения использовать зафиксированные команды CI (`cargo fmt --all -- --check`, locked workspace check/test и Clippy); PostgreSQL проверки запускать только на disposable локальных БД через `scripts/test.sh`. Смотреть `docs/TEST_QUALITY.md`; число тестов не заменяет проверку инвариантов.

<!-- NEDOBOT_REFACTOR_GUIDANCE_BEGIN -->
## Архитектурные правила для новых изменений (обязательные)

Этот раздел дополняет существующие правила `AGENTS.md`; при конфликте с историческими указаниями «добавить новый handler в общий command_handler.rs» **этот раздел приоритетен**. Существующие privacy/security/Telegram delivery/SQL/deploy ограничения не ослабляются. Перед рефактором ознакомиться с `docs/CODE_STYLE.md` и `docs/TEST_QUALITY.md`; оба документа — нормативные для code review.

### Расширяемость и границы
- `main.rs`, Telegram router и общие command/callback adapters должны быть **тонкими**: Telegram update → проверка scope/полномочий → typed use-case → typed result → delivery. Новую фичу оформлять отдельным модулем; не добавлять очередную цепочку условий в монолитный handler.
- `dptree` использовать только для маршрутизации и typed dependency injection. Его `.branch` **не** означает broadcast/fan-out. Обязательные шаги и несколько независимых потребителей выражать явно, а durable работа — через job/outbox.
- Не делать универсальный actor/ECS framework, глобальный event bus, service locator, registry или SQL jobs table только ради сокращения числа файлов. Новая абстракция обосновывается минимум двумя реальными потребителями либо конкретной тестовой/операционной проблемой.
- Бизнес-логика не должна импортировать Telegram bot client для принятия чистых решений. Domain DTO/policy/validators отделять от I/O; не строить blanket traits для каждой функции.

### Типизация и enum
- Для конечных наборов состояний, действий, решений, ошибок, провайдерских capabilities и типов событий **предпочитать `enum`**. Ветвление исчерпывающим `match`; недопустим wildcard `_` там, где появление нового варианта требует решения разработчика.
- Не использовать magic strings или integer codes в core-логике (`"pending"`, `"retry_wait"`, `"denied"`), если это типизированный домен. String ↔ enum исключительно на SQL/JSON/protocol boundary; некорректный persisted value — явная ошибка, не silent fallback.
- Предпочитать суммарные типы невозможным комбинациям `bool`/`Option`; typed newtypes для разнородных ID, версий, source scope и revision, когда это устраняет ошибку смешения.
- `Result<T, DomainError>` для expected domain failures; `anyhow` допустим в composition binaries/CLI и для контекстов, где типизировать каждый уровень вреднее. Не классифицировать ошибки поиском слов в `error.to_string()`.
- `#[serde(deny_unknown_fields)]` и fail-closed для input от внешних источников там, где семантика требует строгой схемы. Не навязывать строгую схему всем Telegram raw events без проверки расширяемости Bot API.

### Сначала зависимости, потом собственный код
- **Запрещено писать велосипед вместо существующей поддерживаемой зависимости** для URL/IDNA/PSL, Telegram API/entities, HTTP retries/limiting, MIME, QR, JSON/MCP transport, timezones, structured output, workflow cancellation и test generation без явной ADR.
- Порядок: проверить уже подключённый crate / API → провести поиск по crates.io/docs.rs/GitHub → сравнить кандидатов (maintainership, API, лицензия, безопасность, transitive deps, portability, поведение на ошибках) → либо reuse, либо короткая документированная реализация. Не тащить тяжёлую dependency ради простой 5-строчной чистой функции.
- Не копировать стороннюю реализацию внутрь репозитория, не строить собственный `yt-dlp`/FFmpeg/Teloxide/MCP/URL parser. Любой внешний процесс — через проверенный bounded adapter без shell interpolation и leakage.

### Качество тестирования (обязательное, не количество ради количества)
- Каждый PR с нетривиальным поведением должен показать **что именно тест способен поймать**: invariant, counterexample и/или искусственный broken implementation. Не принимать тесты вида `assert!(true)`, проверки лишь формы JSON, совпадение объекта с самим собой, тест с воспроизведением того же алгоритма вместо независимого оракула.
- Unit + table-driven edge cases + property/metamorphic tests (`proptest`) + deterministic replay/shrink + integration/concurrency tests (для SQL и lifecycle), по области изменений. Все adversarial cases synthetic, не помещать реальные Telegram ссылки/чаты/токены в репозиторий.
- Использовать **фиксированный воспроизводимый seed**, выводить минимальный контрпример и точную команду replay; отдельные случайные seed допускаются как дополнение. Защититься от false passes: осознанно ввести один локальный дефект и доказать падение соответствующего теста либо выполнить выборочное mutation testing.
- При URL/Telegram parsing, hidden entities, QR, edit/delete/replay проверять не только `count`, но и identity, provenance, dedup, privacy, idempotency, canonicalization, false positive/negative, scope и boundary. Не считать большой corpus доказательством качества без оракула.
- SQL transitions тестировать на двух конкурирующих claims/lease loss, revision changes, stale finalization, crash between external effect and commit. `delivery_unknown` не ретраить автоматически. Тесты без настоящего PostgreSQL не заменяют PG integration tests.
- Формат/Clippy/tests по матрице `docs/TEST_QUALITY.md` и текущей CI. Не сообщать «прошло», если не запускал; указывать число исполненных/пропущенных кейсов, реальную команду, конкретные ограничения environment.

### Переписывание и дисциплина
- Перед перемещением code path добавить characterization tests. Отдельный PR — отдельное смысловое изменение. Рефактор не меняет visible behavior, score, санкции, URL shadow или outbound fallback. Обнаруженный баг исправлять отдельным `fix` PR.
- SQL migrations **только новые additive файлы**; не менять уже применённые и не включать prod data в тесты. Сохранять fencing, write order, `not_spam`, privacy scopes, authority boundaries и exact provider routing.
- Short-form video import / Shorts / TikTok — **не реализовывать**, использовать только как hypothetical design test: новый модуль не должен требовать правок Ask, Voice или URL audit.
### Доказательства и privacy review

- Ревью: разделять подтверждённый дефект (точный ref, code path, нарушенный контракт, counterexample) и гипотезу/технический долг. LOC, число inline tests, отсутствие прямого вызова, historical changelog и workspace membership сами по себе не доказывают дефект.
- Test plan привязать к реальному инварианту, не к форме реализации. В PR указывать команды, exit codes, passed/failed/ignored и причины пропуска; при недоступном tooling — явно написать not run, не придумывать coverage, fuzz/mutation score или результат CI.
- Для security/permission paths проверять матрицу public/private/ephemeral, managed/foreign chat и instance, роли и opt-in, reply target и feature gates. Fail closed; при ошибке API/рендера/LLM не расширять audience, scope, доступ к истории или допустимые инструменты.
- Недоверенные Telegram/LLM/MCP/URL данные не являются инструкциями для agent/runtime. Scope, авторизация, проверка аргументов и provenance enforced кодом до выдачи данных/эффекта, не только prompt. Не логировать скрытые targets, query, raw body, private memory и secrets; hash не равен анонимизации.
- Dependency/advisory: проверять actual locked package, affected range, класс advisory, patched version и dependency path по upstream. Обновлять lockfile штатным Cargo и тестировать; exception только узкий, с доказанной границей воздействия, причиной, условием удаления и отдельным review. Не выдавать устранение одного advisory за зелёный полный audit.
- PR не равен релизу: пользовательские изменения готовят для changelog по `docs/DEVELOPMENT.md`, дату выпуска фиксируют только при фактическом deploy. Не менять production, не merge и не deploy без отдельной авторизации.

<!-- NEDOBOT_REFACTOR_GUIDANCE_END -->
