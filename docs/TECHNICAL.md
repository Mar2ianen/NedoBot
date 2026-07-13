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
- Автоматически конспектирует обычные новости в память и подмешивает релевантные заметки в следующие генерации.
- Объединяет похожие заметки памяти, чтобы не плодить дубли.
- Подмешивает последние ответы бота в prompt, чтобы не повторять одинаковые CTA.
- Опционально добавляет свежий web/GitHub/Reddit факт-чек для первого комментария через lazy MCP process, если включён `SEARCH_ENABLED`.
- Собирает статистику чата с дневной/недельной/месячной отсечкой в 05:00 МСК.
- Показывает пользователей в отчётах человекочитаемо: имя кликабельно, ID спрятан в `tg://user`, рядом статус/админство.
- Сохраняет новые reaction updates, reaction count updates и chat member updates, если Telegram отдаёт их боту.
- Расшифровывает `voice`, `audio` и `video_note`, если включены `VOICE_TRANSCRIPTION_ENABLED` и `VOICE_AUTO_TRANSCRIBE`.
- Для аудиозаписей делает Groq ASR, LLM cleanup, safe Telegram HTML render и audit в `voice_transcription_jobs`.
- Короткие расшифровки отправляет plain text без глав/таймкодов; длинные может отправлять главами с expandable blockquotes или preview + `.txt` файлом.

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

Локальный `.env` не коммитится. Шаблон лежит в [.env.example](../.env.example).

Основные переменные:

```env
TELOXIDE_TOKEN=
DATABASE_URL=postgres://tg_ai_bot:tg_ai_bot@localhost:5432/tg_ai_bot

SOURCE_CHANNEL_ID=-1001575496091
DISCUSSION_CHAT_ID=-1001932061163
CHAT_INVITE_URL=https://t.me/+RxmPtw7Bs-IxNzEy
CHAT_INVITE_LABEL=Присоединяйтесь к чату
POST_SIGNATURE_MARKER=Не теряем связь

LLM_PROVIDER=gemini
LLM_MODEL=gemini-3.5-flash
LLM_SUPPORTS_IMAGES=true
LLM_TEMPERATURE=0.45
LLM_MAX_TOKENS=90
LLM_PROXY_URL=
MEMORY_LLM_TEMPERATURE=0.2
MEMORY_LLM_MAX_TOKENS=220

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
SEARCH_MCP_TOOL_FETCH=web_fetch_exa
SEARCH_FETCH_TOP_N=2
SEARCH_FETCH_MAX_CHARS=6000
SEARCH_GITHUB_MCP_COMMAND=
SEARCH_GITHUB_MCP_ARGS=
SEARCH_GITHUB_MCP_ENV=PATH,HOME,GITHUB_PERSONAL_ACCESS_TOKEN
SEARCH_GITHUB_MCP_TOOLS=search_issues,search_code

GROQ_API_KEY=
GROQ_MODEL=
CEREBRAS_API_KEY=
CEREBRAS_MODEL=
OPENROUTER_API_KEY=
OPENROUTER_MODEL=
GEMINI_API_KEY=
GEMINI_TEXT_MODEL=gemini-3.5-flash
GEMINI_FLASH_MODEL=gemini-3.1-flash-lite
GEMINI_TTS_MODEL=gemini-3.1-flash-tts-preview
GEMINI_THINKING_BUDGET=1024

PUBLIC_BASE_URL=
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
PROFILE_REFRESH_CONCURRENCY=4
```

Для комментариев рекомендуемый основной provider — `gemini`: `Gemini 3.5 Flash` как основная модель, `Gemini 3.1 Flash Lite` как первый fallback, затем `ollama`/`gemma4:31b` как последний fallback. Fallback-цепочка срабатывает только когда модель не переопределена явно на уровне конкретного вызова.

Если Gemini недоступен напрямую из региона сервера, `LLM_PROXY_URL` может направить только LLM/Gemini-запросы через HTTP/SOCKS proxy, не трогая Telegram polling. На текущем `vps-153` Gemini-трафик идёт через `LLM_PROXY_URL=socks5h://127.0.0.1:2080`, который поднимает systemd-сервис `gemini-proxy-ssh.service` SSH-туннелем до `vps-85`.

Для reasoning Gemini-моделей `GEMINI_THINKING_BUDGET` задаёт отдельный бюджет thinking-токенов. В Gemini API общий `maxOutputTokens` включает и thinking, и финальный ответ, поэтому бот отправляет `maxOutputTokens = LLM_MAX_TOKENS + GEMINI_THINKING_BUDGET`, а длину/качество финального комментария дополнительно контролирует output validator.

На старте основной сервис и `retry_pending_comments` делают fail-fast проверку секретов для включённых функций:

- `LLM_PROVIDER=gemini` требует непустой `GEMINI_API_KEY` или `GOOGLE_AI_STUDIO_API_KEY`.
- `LLM_PROVIDER=groq|cerebras|openrouter|openai_compat` требует соответствующий API key.
- `LLM_PROVIDER=groq|cerebras|openrouter` требует явную модель через `LLM_MODEL` или provider-specific переменную `GROQ_MODEL`/`CEREBRAS_MODEL`/`OPENROUTER_MODEL`; fallback на `VISION_MODEL` запрещён.
- `LLM_PROVIDER=ollama` секрета не требует.
- Если включены `VOICE_TRANSCRIPTION_ENABLED=true` и `VOICE_AUTO_TRANSCRIBE=true`, `VOICE_ASR_PROVIDER=groq` требует `GROQ_API_KEY`.
- Если для включённого voice pipeline задан `VOICE_CLEANUP_PROVIDER`, для него тоже проверяется соответствующий LLM secret.

Это специально ловит ситуацию, когда конфиг переключили на Gemini, но ключ на сервере пустой: бот не стартует с тихим уходом в fallback.

### Поиск фактов для первого комментария

SEARCH-контур добавляет вспомогательный свежий контекст перед генерацией первого комментария:

```text
clean post -> extract JSON queries -> lazy MCP process -> SearchContext -> build_llm_prompt -> generate_text_checked
```

Поведение gated by config:

- `SEARCH_ENABLED=false` сохраняет старое поведение: search-блок не добавляется в prompt, а генерация идёт через обычный `LLM_PROVIDER` без внешнего поиска.
- `SEARCH_EXTRACT_PROVIDER` / `SEARCH_EXTRACT_MODEL` задают LLM, который из очищенного поста возвращает JSON с максимум 3 запросами для `web`, `github` или `reddit`.
- `SEARCH_MCP_COMMAND` и `SEARCH_MCP_ARGS` запускают основной MCP server лениво на один search-run. Long-lived MCP client в `AppState`, lifecycle restart/shutdown и постоянный child process не используются в первой итерации.
- `SEARCH_MCP_ENV` — allowlist имён env vars, которые можно передать MCP child process. Значения не логируются.
- `SEARCH_MCP_TOOL_WEB`, `SEARCH_MCP_TOOL_GITHUB`, `SEARCH_MCP_TOOL_REDDIT` задают имена MCP tools для основного MCP server.
- `SEARCH_MCP_TOOL_FETCH` включает дополнительный fetch top URL после search. Для Exa это `web_fetch_exa`.
- `SEARCH_GITHUB_MCP_COMMAND` / `SEARCH_GITHUB_MCP_ARGS` включают отдельный GitHub MCP server для запросов `source=github`; если они не заданы, GitHub-запросы идут через основной `SEARCH_MCP_TOOL_GITHUB`.
- `SEARCH_GITHUB_MCP_ENV` по умолчанию пропускает только `PATH,HOME,GITHUB_PERSONAL_ACCESS_TOKEN`; значения не логируются.
- `SEARCH_GITHUB_MCP_TOOLS` по умолчанию вызывает только read-only `search_issues,search_code`; write tools GitHub MCP не вызываются.
- Для GitHub results бот дополнительно дочитывает top-N URL через read-only `get_issue` / `get_file_contents`: issue/PR body, `README.md`, `CHANGELOG.md`, release docs и другие blob-файлы попадают в snippet как `Fetch: ...`.
- `SEARCH_FETCH_TOP_N` ограничивает число URL для fetch, `SEARCH_FETCH_MAX_CHARS` — объём текста на страницу.
- Любая ошибка extract/MCP/parsing/timeout превращается в skipped `SearchContext`, комментарий не ломается.
- Результаты поиска добавляются в JSON-контекст без raw URL и имеют приоритет ниже текста поста. Первые fetched-результаты получают до 6000 символов каждый, общий бюджет search-контекста — 14 000 символов; URL остаётся только в `SearchContext` для безопасного рендера.
- Каждый search-run сохраняется в `search_runs` для аналитики: статус, skipped reason, latency, queries/results как `jsonb`. Кэша результатов пока нет — запись аналитическая, не влияет на генерацию.

Проверенный вариант без отдельного API key — hosted Exa MCP через `mcp-remote`:

```env
SEARCH_ENABLED=true
SEARCH_MCP_COMMAND=npx
SEARCH_MCP_ARGS="-y mcp-remote https://mcp.exa.ai/mcp"
SEARCH_MCP_ENV=PATH,HOME
SEARCH_MCP_TIMEOUT_SEC=30
SEARCH_MCP_TOOL_WEB=web_search_exa
SEARCH_MCP_TOOL_GITHUB=web_search_exa
SEARCH_MCP_TOOL_REDDIT=web_search_exa
SEARCH_MCP_TOOL_FETCH=web_fetch_exa
SEARCH_FETCH_TOP_N=2
SEARCH_FETCH_MAX_CHARS=6000
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

- `VOICE_TRANSCRIPTION_ENABLED=false` полностью выключает voice pipeline.
- `VOICE_AUTO_TRANSCRIBE=false` оставляет контур выключенным для обычных сообщений; ручной `/transcribe` пока не реализован.
- `VOICE_ASR_PROVIDER=groq` - сейчас единственный поддержанный ASR provider.
- `VOICE_ASR_MODEL=whisper-large-v3` - дефолт для точной мультиязычной расшифровки в пределах Free Plan лимитов Groq.
- `VOICE_CLEANUP_PROVIDER` пустой значит использовать обычный `LLM_PROVIDER`.
- `VOICE_CLEANUP_MODEL` пустой значит использовать модель обычного provider-а.
- `VOICE_SHORT_TEXT_MAX_CHARS=400` значит короткая расшифровка после cleanup отправляется как простой текст без глав и времени.
- `VOICE_MAX_FILE_MB=20` выбран под cloud Bot API `getFile`; для больших файлов нужен local Bot API server.
- Если обычный HTML не влезает в безопасный лимит Telegram, бот отправляет Rich Message с закрытым блоком полного текста. `VOICE_SEND_FULL_FILE=true` оставляет `preview + voice-transcript.txt` только как fallback при ошибке Rich API или превышении rich-лимита.

## Локальный Запуск

Поднять Postgres:

```bash
docker compose up -d postgres
```

Запустить бота:

```bash
cargo run
```

Проверка:

```bash
cargo check
```

## VPS Деплой

Текущий тестовый деплой сделан на `vps-153`:

- код: `/opt/tg-ai-bot-teloxide`
- Postgres: Podman container `tg-ai-bot-postgres`
- systemd:
  - `container-tg-ai-bot-postgres.service`
  - `tg-ai-bot-teloxide.service`

Полезные команды:

```bash
ssh vps-153 'systemctl status tg-ai-bot-teloxide --no-pager'
ssh vps-153 'journalctl -u tg-ai-bot-teloxide -f'
ssh vps-153 'podman ps'
```

Ручной redeploy из локальной папки:

```bash
rsync -az --delete --exclude target --exclude .git --exclude .env ./ vps-153:/opt/tg-ai-bot-teloxide/
ssh vps-153 'cd /opt/tg-ai-bot-teloxide && /root/.cargo/bin/cargo build --release && systemctl restart tg-ai-bot-teloxide'
```

## База

Главные таблицы:

- `telegram_messages` - входящие сообщения и raw Telegram JSON.
- `post_comment_jobs` - дедупликация и статус комментария под постом.
- `llm_generations` - prompt, модель, ответ LLM и финальный HTML.
- `post_memory_notes` - короткие конспекты прошлых новостей, keywords и осторожные ограничения для будущих комментариев.
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

Посмотреть последние сообщения:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select chat_id, message_id, source_channel_id, source_message_id, is_automatic_forward, left(coalesce(text, ''), 200) as text, created_at from telegram_messages order by id desc limit 20;\""
```

Посмотреть задачи комментариев:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select * from post_comment_jobs order by id desc limit 20;\""
```

Посмотреть память:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select title, summary, keywords, created_at from post_memory_notes order by id desc limit 20;\""
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

`/userstats` принимает числовой Telegram ID, уже виденный ботом username или reply на сообщение пользователя. Без аргумента команда показывает отправителя. В общих отчётах ID намеренно не печатается; для точного SQL-разбора он остаётся в таблицах `telegram_messages`, `telegram_user_profiles` и `telegram_chat_users`.

## Prompt

Основной prompt лежит в [prompts/first_comment.md](../prompts/first_comment.md).
Короткий факт-чек/RAG для защиты от устаревших утверждений лежит в [prompts/tech_rag.md](../prompts/tech_rag.md).
Cleanup prompt для расшифровки голосовых лежит в [prompts/voice_cleanup.md](../prompts/voice_cleanup.md).

Модель первого комментария возвращает structured JSON: `{"comment":"...","used_search_result_id":null}`. В `comment` обязателен ровно один `{CHAT_LINK}` или вариант с разрешённым текстом ссылки вроде `{CHAT_LINK:чате}` / `{CHAT_LINK:комментах}`. Gemini получает JSON Schema через API, Ollama fallback — ту же schema через `format`; для остальных совместимых провайдеров сохраняется строгий JSON-контракт в prompt.

Если поиск вернул результат с публичным HTTP(S) URL, модель ищет отдельный угол, которого нет в новости: связанный релиз, ограничение, последствие, сравнение, цену, changelog или реакцию сообщества. Поиск нельзя использовать только для подтверждения или пересказа факта из поста. Если полезного дополнения нет, модель возвращает `used_search_result_id: null` и пишет уникальную реплику по самой новости. Если внешний факт использован, его one-based ID сохраняется в `llm_generations`, а `{SOURCE_LINK:N:короткая подпись}` становится обязательным. Output validator отклоняет факт без источника, raw URL, битые/лишние плейсхолдеры, неподходящий ID, текст длиннее 180 видимых символов и generic CTA. Код сам рендерит ссылки в HTML, а предпросмотр ссылок отключён для обычных и rich text send-путей.
RAG не предназначен для пересказа новости: пост канала важнее, а карточки нужны только чтобы не писать ложные вещи вроде `Switch 2 еще не вышла`.

Автоматическая память работает поверх RAG:

- после отправки первого комментария бот просит LLM сделать короткую заметку по посту;
- заметка сохраняется в `post_memory_notes` с keywords;
- если уже есть похожая заметка, бот обновляет её вместо создания дубля;
- похожесть сейчас считается по пересечению keywords, минимум 3 общих ключа;
- короткие ключи вроде `sam` матчятся только как отдельное слово, чтобы не склеивать `SAM` и `Samsung`;
- перед новой генерацией бот достаёт до 5 похожих заметок по пересечению keywords;
- память используется только как контекст, если она релевантна текущему посту.

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

1. Проверить `VOICE_TRANSCRIPTION_ENABLED` и `VOICE_AUTO_TRANSCRIBE`.
2. Отфильтровать чужие чаты, ботов, команды и automatic forwards.
3. Определить `VoiceMedia` из `voice`, `audio` или `video_note`.
4. Сохранить исходное Telegram message в `telegram_messages`.
5. Создать `voice_transcription_jobs`; повтор того же `(chat_id, message_id)` не создаёт новый job.
6. Проверить duration/file size до скачивания.
7. Скачать файл через Telegram `getFile` во временный файл.
8. Для `video_note` задать multipart MIME `video/mp4` и отправить исходный MP4 в Groq `/audio/transcriptions`.
9. Сохранить raw ASR text, segments и raw JSON.
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
- `Завлечение после коммента` считает среднее число некомандных сообщений после комментария бота за 5 и 30 минут, плюс среднее число уникальных людей за 30 минут.
- `Комменты бота` сортируются по обсуждению за 30 минут, прямым реплаям и реакциям. Текст очищается от HTML/AI-маркеров и обрезается до короткого превью.

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

- Поиск памяти keyword-based, без embeddings/pgvector.
- Merge памяти эвристический; за качеством заметок надо иногда смотреть через `/memory` или SQL.
- Реакции считаются только с момента включения reaction updates; старые реакции Telegram Bot API задним числом не отдаёт.
- Статусы пользователей известны по последнему `chat_member` update или по будущим снимкам; если Telegram не присылал событие, статус будет `unknown`.
- Если LLM provider вернёт ошибку/subscription limit, задача может остаться без комментария до ручного вмешательства.
- Voice ASR сейчас только через Groq; local Whisper/Ollama audio не подключены.
- Cleanup provider/model для voice пока не сохраняются в отдельные DB-поля, хотя поля в таблице уже есть.
- Join-конверсия по отдельной invite-ссылке пока не считается автоматически.
- Админки пока нет; настройки меняются через `.env` и рестарт сервиса.
