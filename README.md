# TG AI Bot Teloxide

Telegram-бот на Rust/teloxide для `НедоNews Chat`.

Текущая MVP-задача: бот ждёт авто-форвард поста из канала в привязанном чате, проверяет что это обычный пост с подписью `Не теряем связь`, вырезает VK/MAX-хвост, отдаёт текст и фото в Ollama Gemma, затем отвечает первым комментарием под постом.

## Что Уже Работает

- Читает сообщения из `НедоNews Chat`, если privacy mode выключен до добавления бота в чат.
- Сохраняет входящие сообщения в Postgres.
- Распознаёт авто-форварды из канала по `forward_origin.channel.id`.
- Пропускает рекламу/служебные посты без маркера `Не теряем связь`.
- Скачивает самое большое фото поста и отправляет его в vision-модель.
- Генерирует комментарий через Ollama Cloud `gemma4:31b`.
- Отправляет HTML-комментарий reply под постом.
- Отключает link preview.
- Подставляет premium/custom emoji по тематике, включая канал/AMD/Radeon/Ryzen.
- Пишет задачи и результаты генерации в Postgres.
- Автоматически конспектирует обычные новости в память и подмешивает релевантные заметки в следующие генерации.

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

Локальный `.env` не коммитится. Шаблон лежит в [.env.example](.env.example).

Основные переменные:

```env
TELOXIDE_TOKEN=
DATABASE_URL=postgres://tg_ai_bot:tg_ai_bot@localhost:5432/tg_ai_bot

SOURCE_CHANNEL_ID=-1001575496091
DISCUSSION_CHAT_ID=-1001932061163
CHAT_INVITE_URL=https://t.me/+RxmPtw7Bs-IxNzEy
CHAT_INVITE_LABEL=чате
POST_SIGNATURE_MARKER=Не теряем связь

OLLAMA_API_KEY=
OLLAMA_BASE_URL=https://ollama.com
VISION_MODEL=gemma4:31b

OWNER_TELEGRAM_ID=
SEND_OWNER_PREVIEW=true
```

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
- `bot_settings`, `telegram_users`, `telegram_chats`, `admin_events` - задел под админку.

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

## Команды Бота

```text
/ping
/db
/emojiids
/format_test <текст поста>
/memory
```

В группах лучше писать с username:

```text
/ping@nedostraj_bot
```

## Prompt

Основной prompt лежит в [prompts/first_comment.md](prompts/first_comment.md).
Короткий факт-чек/RAG для защиты от устаревших утверждений лежит в [prompts/tech_rag.md](prompts/tech_rag.md).

Важно: модель должна вернуть текст с плейсхолдером `{CHAT_LINK}`. Код сам заменяет его на HTML-ссылку и не даёт модели портить URL.
RAG не предназначен для пересказа новости: пост канала важнее, а карточки нужны только чтобы не писать ложные вещи вроде `Switch 2 еще не вышла`.

Автоматическая память работает поверх RAG:

- после отправки первого комментария бот просит LLM сделать короткую заметку по посту;
- заметка сохраняется в `post_memory_notes` с keywords;
- если уже есть похожая заметка, бот обновляет её вместо создания дубля;
- перед новой генерацией бот достаёт до 5 похожих заметок по пересечению keywords;
- память используется только как контекст, если она релевантна текущему посту.

## Custom Emoji

Список считанных premium/custom emoji:

- [docs/custom_emoji_stickers.tsv](docs/custom_emoji_stickers.tsv)
- [docs/custom_emoji_sheet.png](docs/custom_emoji_sheet.png)

Текущие ID:

```env
COMMENT_CUSTOM_EMOJI_ID=5445092965875729965
AMD_CUSTOM_EMOJI_ID=5442995600201106682
RADEON_CUSTOM_EMOJI_ID=5442853853395436819
RYZEN_CUSTOM_EMOJI_ID=5444875271163364561
```

## Ограничения MVP

- Автокоммент проверен инфраструктурно, но ждёт реальный новый пост канала.
- Если Ollama Cloud вернёт ошибку/subscription limit, задача останется без комментария до ручного вмешательства.
- Админки пока нет; настройки меняются через `.env` и рестарт сервиса.
