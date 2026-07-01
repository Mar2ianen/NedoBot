# Completed refactor state

Архив того, что уже закрыто большим проходом рефактора. Активный следующий план лежит в [`REFACTOR_NEXT.md`](REFACTOR_NEXT.md).

## Итоговое состояние

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

## Что было закрыто

- [x] `telegram/html.rs` добавлен.
- [x] `telegram/render.rs` централизует HTML send и link preview disable.
- [x] First-comment renderer переведён на `Html`.
- [x] Memory report переведён на `Html`.
- [x] Stats partially переведён на безопасный HTML.
- [x] First-comment prompt/repo/pipeline вынесены из `main.rs`.
- [x] Telegram persistence вынесен из `main.rs`.
- [x] LLM routing вынесен и нормализует provider/model/image_used.
- [x] Unknown `LLM_PROVIDER` больше не падает молча в Ollama.
- [x] Best-effort image download не ломает text-only генерацию.
- [x] `ChatStatsSummary` заменён с tuple на struct.
- [x] Telegram export importer добавлен.
- [x] Pending comment retry tool добавлен.
- [x] Member refresh tool добавлен.
- [x] `/topmsg` и `/topreact` добавлены.

## Инварианты, которые нельзя ломать

- Бот не отвечает на обычные сообщения чата первым комментарием.
- First-comment candidate только для auto-forward из `SOURCE_CHANNEL_ID` в `DISCUSSION_CHAT_ID`.
- Реклама/служебные посты без `POST_SIGNATURE_MARKER` пропускаются.
- Модель не владеет HTML-ссылкой на чат: `{CHAT_LINK}` или fallback заменяются кодом.
- Любой пользовательский или модельный текст перед HTML должен проходить через `Html::text`, `Html::bold`, `Html::code`, `Html::link` или `escape_html`.
- `raw_trusted` использовать только для тегов, собранных кодом.
- Длинные сообщения не отправлять в Telegram вслепую.
- Исторический импорт должен быть повторяемым: повторный запуск не должен раздувать счётчики.

## Команды после рефактора

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

## Operational smoke checks

После деплоя:

```bash
ssh vps-153 'systemctl status tg-ai-bot-teloxide --no-pager'
ssh vps-153 'journalctl -u tg-ai-bot-teloxide -n 120 --no-pager'
```

Последние сообщения:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select chat_id, message_id, source_channel_id, source_message_id, is_automatic_forward, left(coalesce(text, ''), 160) as text, created_at from telegram_messages order by id desc limit 20;\""
```

First-comment jobs:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select id, status, source_message_id, discussion_message_id, bot_comment_message_id, error, created_at, updated_at from post_comment_jobs order by id desc limit 20;\""
```

LLM generations:

```bash
ssh vps-153 "podman exec tg-ai-bot-postgres psql -U tg_ai_bot -d tg_ai_bot -P pager=off -c \"select post_comment_job_id, provider, model, image_used, left(response, 120) as response, left(final_html, 120) as final_html, created_at from llm_generations order by id desc limit 10;\""
```

## Maintenance commands

Импорт истории:

```bash
cargo run --bin import_telegram_export -- "/path/to/ChatExport/result.json" --dry-run
cargo run --release --bin import_telegram_export -- "/path/to/ChatExport/result.json"
```

Username aliases из реального Telegram ID:

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

Обновление аватарок, bio и raw profile details:

```bash
cargo run --release --bin refresh_user_profiles -- --limit 200
cargo run --release --bin refresh_user_profiles -- --only-spammers --limit 200
cargo run --release --bin refresh_user_profiles -- --all --limit 500 --sleep-ms 100
```

Новые авторы сообщений в основном чате профилируются сразу при первом увиденном сообщении, если `profile_refreshed_at` ещё пустой. CLI остаётся для добивки старой истории и ручного прохода по спамерам.

Telegram отдаёт bio и фото best-effort: часть пользователей может иметь пустой bio, закрытый профиль или ноль публичных фото. Ошибка API пишется в `telegram_user_profiles.profile_refresh_error`.

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
