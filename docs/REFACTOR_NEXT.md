# Current state and next steps

Рабочая карта после большого прохода по рефактору, импорту истории и статистике. Старый план `telegram/html.rs -> first_comment/pipeline.rs -> import later` уже в основном выполнен, поэтому этот документ фиксирует текущее состояние и ближайшие безопасные шаги.

## Текущий статус

Проект уже не выглядит как один большой `main.rs`. Сейчас это нормальный MVP-монолит с понятными контурами:

- `main.rs` отвечает за init, migrations, dispatcher wiring и маленькие update handlers.
- `config.rs` читает `.env` и держит runtime-настройки.
- `state.rs` прокидывает `PgPool` и `Config` в handlers.
- `db/telegram.rs` сохраняет live Telegram updates, профили, реакции, member snapshots и activity counters.
- `telegram/html.rs` отвечает за безопасную сборку Telegram HTML.
- `telegram/render.rs` отвечает за отправку HTML, отключение previews, empty fallback и hard-limit `4096` chars.
- `telegram/command_handler.rs` держит команды и делегирует в features.
- `features/first_comment/*` держит candidate detection, cleaning, prompt, renderer, repository и pipeline первого комментария.
- `features/memory/*` держит память новостей и отчёт `/memory`.
- `features/stats/*` держит отчёты, user stats, top messages и top reactions.
- `llm/*` держит Ollama и OpenAI-compatible routing.
- `src/bin/import_telegram_export.rs` импортирует Telegram Desktop/AyuGram export.
- `src/bin/retry_pending_comments.rs` ретраит зависшие first-comment jobs.
- `src/bin/refresh_chat_members.rs` добивает профили и member snapshots через `getChatMember`.

## Последние важные изменения

После исходного рефактор-плана добавлены:

- безопасный HTML builder и централизованная отправка HTML;
- hard-fail для Telegram HTML длиннее `4096` символов;
- защита от пустого first-comment HTML, чтобы бот не отправлял `Пустой ответ.` под постом;
- retry tool для pending comment jobs;
- lib target `src/lib.rs`, чтобы maintenance binaries могли переиспользовать модули основного бота;
- импорт Telegram export в `telegram_messages`, `telegram_user_profiles`, `telegram_chat_users`, reaction counts и recent reaction events;
- dedup для reaction events;
- refresh tool для user profiles/member snapshots;
- `/userstats` без аргумента выбирает отправителя, а reply выбирает автора сообщения;
- `/topmsg` и `/topreact` для исторической статистики;
- улучшенное разрешение имён пользователей из profiles и raw export JSON;
- дефолтный LLM provider возвращён к Ollama/Gemma.

## Инварианты

Не ломать эти свойства без отдельного осознанного коммита:

- Бот не отвечает на обычные сообщения чата первым комментарием.
- First-comment candidate только для auto-forward из `SOURCE_CHANNEL_ID` в `DISCUSSION_CHAT_ID`.
- Реклама/служебные посты без `POST_SIGNATURE_MARKER` пропускаются.
- Модель не владеет HTML-ссылкой на чат: `{CHAT_LINK}` или fallback заменяются кодом.
- Любой пользовательский или модельный текст перед HTML должен проходить через `Html::text`, `Html::bold`, `Html::code`, `Html::link` или `escape_html`.
- `raw_trusted` использовать только для тегов, собранных кодом.
- Длинные сообщения не отправлять в Telegram вслепую.
- Исторический импорт должен быть повторяемым: повторный запуск не должен раздувать счётчики.

## Operational smoke checks

После деплоя:

```bash
ssh vps-153 'systemctl status tg-ai-bot-teloxide --no-pager'
ssh vps-153 'journalctl -u tg-ai-bot-teloxide -n 120 --no-pager'
```

Проверить последние сообщения:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select chat_id, message_id, source_channel_id, source_message_id, is_automatic_forward, left(coalesce(text, ''), 160) as text, created_at from telegram_messages order by id desc limit 20;\""
```

Проверить first-comment jobs:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select id, status, source_message_id, discussion_message_id, bot_comment_message_id, error, created_at, updated_at from post_comment_jobs order by id desc limit 20;\""
```

Проверить LLM generations:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select post_comment_job_id, provider, model, image_used, left(response, 120) as response, left(final_html, 120) as final_html, created_at from llm_generations order by id desc limit 10;\""
```

## Maintenance commands

Импорт истории:

```bash
cargo run --bin import_telegram_export -- "/path/to/ChatExport/result.json" --dry-run
cargo run --release --bin import_telegram_export -- "/path/to/ChatExport/result.json"
```

Если нужны username aliases из реального Telegram ID:

```bash
cargo run --release --bin import_telegram_export -- "/path/to/ChatExport/result.json" --user-alias username:123456789
```

Ретрай зависших комментариев:

```bash
cargo run --release --bin retry_pending_comments -- --limit 10
```

Обновление профилей и member snapshots:

```bash
cargo run --release --bin refresh_chat_members -- --limit 200
cargo run --release --bin refresh_chat_members -- --all --limit 500 --sleep-ms 80
```

## Current command surface

Публичные/полупубличные команды:

```text
/help
/ping
/db
/emojiids
/format_test <текст поста>
/memory
/stats_day
/stats_week
/stats_month
/topmsg
/topreact
/userstats <id|username>
```

Поведение:

- `/userstats` без аргумента показывает отправителя команды.
- `/userstats` reply на сообщение показывает автора reply.
- `/topmsg` показывает топ пишущих пользователей за всё время.
- `/topreact` показывает топ сообщений по reaction counts со ссылками на сообщения.
- В группах команды лучше писать с username бота, если Telegram ambiguity мешает dispatch.

## Known caveats

### Migrations and reaction dedup

Есть две миграции вокруг reaction-event dedup:

```text
20260701123000_reaction_event_dedup.sql
20260701133000_deduplicate_reaction_events.sql
```

Если база уже успешно прошла первую миграцию, всё нормально. Если продовая база падала именно на создании unique index из-за дублей, более поздняя dedup-миграция сама по себе не выполнится, потому что SQLx остановится на первой неприменённой миграции. В таком случае нужно руками выполнить dedup SQL или аккуратно чинить историю миграций на этой конкретной базе.

Проверка дублей:

```sql
select chat_id, message_id, coalesce(user_id,0), coalesce(actor_chat_id,0), event_at, new_reactions, count(*)
from telegram_message_reactions
group by 1,2,3,4,5,6
having count(*) > 1;
```

### Pending comment jobs

Основной pipeline создаёт `post_comment_jobs` до LLM/send. Если LLM, HTML render или Telegram send упали, job может остаться `pending`. Для этого есть `retry_pending_comments`, но на будущее лучше вынести общий `mark_post_comment_failed` в `features/first_comment/repo.rs` и использовать его и в live pipeline, и в retry tool.

### Reaction metrics

`telegram_message_reaction_counts` хорошо показывает сумму реакций на сообщение. `telegram_message_reactions` из export содержит только `recent`, поэтому пользовательская метрика `реакций поставил` по старой истории неполная. Для live updates она становится точнее только после того, как Telegram начал присылать `message_reaction` updates боту.

### Telegram export service messages

Importer сейчас принимает и `message`, и `service`. Это полезно для части истории, но может загрязнять общие счётчики сообщений/active users, если service events не нужно считать как пользовательскую активность. Если статистика начнёт выглядеть странно, следующий шаг — хранить `message_type` отдельной колонкой или фильтровать service records при построении отчётов.

### HTML length

`send_html` теперь не отправляет сообщения длиннее `4096` символов. Для stats это обычно нормально, но для будущей voice transcription нужен splitter: preview в чат + полный `.txt/.md` файлом.

## Near-term TODO

### 1. First-comment failure accounting

Сейчас empty HTML защищён через `ensure_comment_html`, но failure не всегда попадает в БД как явный `failed`. Желательно:

```rust
pub async fn mark_post_comment_failed(
    pool: &PgPool,
    job_id: i64,
    error: &str,
) -> anyhow::Result<()>;
```

И использовать в live pipeline вокруг LLM/render/send.

### 2. Telegram HTML splitter

Нужен общий renderer для длинных ответов:

```rust
RenderedMessage::Single(Html)
RenderedMessage::Chunks(Vec<Html>)
RenderedMessage::PreviewAndFile { preview: Html, full_text: String }
```

Это понадобится для voice transcription и длинных stats.

### 3. Importer hardening

Следующие улучшения импортёра:

- добавить явное хранение `message_type`, если service messages останутся в `telegram_messages`;
- лучше документировать неполноту `recent` reactions;
- добавить summary после импорта: сколько сообщений, users, reaction counts, recent events реально upserted;
- добавить команду проверки дублей перед миграцией/импортом.

### 4. Voice transcription

Будущая структура:

```text
src/features/voice/
  mod.rs
  pipeline.rs
  asr.rs
  render.rs
  types.rs
```

Пайплайн:

1. принять `voice`/`audio`/`video_note`;
2. скачать файл через Bot API или local Bot API;
3. нормализовать audio через ffmpeg при необходимости;
4. отправить в Groq Whisper `verbose_json` с segment timestamps;
5. отдать segments в LLM на чистку ASR;
6. отправить preview в чат, полный текст файлом при превышении лимита.

### 5. Tests

Минимальный следующий набор:

```text
telegram::html::escape/link/code/custom_emoji
telegram::render::normalize_send_text over 4096
first_comment::render placeholder/fallback/strip links
first_comment::pipeline empty html guard
stats::message_url
import_telegram_export id conversion / rich text / alias parsing
```

## Done checklist

- [x] `telegram/html.rs` добавлен.
- [x] `telegram/render.rs` централизует HTML send и link preview disable.
- [x] First-comment renderer переведён на `Html`.
- [x] Memory report переведён на `Html`.
- [x] Stats partially переведён на безопасный HTML.
- [x] First-comment prompt/repo/pipeline вынесены из `main.rs`.
- [x] Telegram persistence вынесен из `main.rs`.
- [x] LLM routing вынесен и нормализует provider/model/image_used.
- [x] Telegram export importer добавлен.
- [x] Pending comment retry tool добавлен.
- [x] Member refresh tool добавлен.
- [x] `/topmsg` и `/topreact` добавлены.

Контрольный вопрос сейчас:

> Можно ли добавить `features/voice` без правок на 300 строк в `main.rs`?

Да. Следующий риск уже не в `main.rs`, а в аккуратном API для длинного Telegram output и в хранении voice transcript artifacts.
