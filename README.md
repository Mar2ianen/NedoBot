# TG AI Bot Teloxide

Telegram-бот на Rust/teloxide для `НедоNews Chat`.

Текущая MVP-задача: бот ждёт авто-форвард поста из канала в привязанном чате, проверяет что это обычный пост с подписью `Не теряем связь`, вырезает VK/MAX-хвост, отдаёт текст, фото, RAG, память и последние ответы в Ollama Gemma, затем отвечает первым комментарием под постом.

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
- Объединяет похожие заметки памяти, чтобы не плодить дубли.
- Подмешивает последние ответы бота в prompt, чтобы не повторять одинаковые CTA.
- Собирает статистику чата с дневной/недельной/месячной отсечкой в 05:00 МСК.
- Показывает пользователей в отчётах человекочитаемо: имя кликабельно, ID спрятан в `tg://user`, рядом статус/админство.
- Сохраняет новые reaction updates, reaction count updates и chat member updates, если Telegram отдаёт их боту.

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
- `telegram_user_profiles` - последние виденные username/name/is_bot/is_premium.
- `telegram_chat_users` - явная расширяемая карточка пользователя в конкретном чате: первое/последнее сообщение, счётчики сообщений/реплаев/ссылок/медиа, статус в чате, админство, join/leave/invite-link поля.
- `telegram_chat_member_snapshots` - последний известный статус пользователя в чате.
- `telegram_chat_member_events` - входы, выходы и изменения статусов, если Telegram прислал update.
- `telegram_message_reactions` - персональные изменения реакций.
- `telegram_message_reaction_counts` - последние известные счётчики реакций по сообщению.
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
/stats_day
/stats_week
/stats_month
/userstats <id|@username>
```

В группах лучше писать с username:

```text
/ping@nedostraj_bot
```

`/stats_day`, `/stats_week` и `/stats_month` показывают имена пользователей как скрытые ссылки на Telegram-профиль, без видимого ID. Рядом выводятся короткие бейджи: `админ`, `в чате`, `не в чате`, `бот` или `статус неизвестен`.

`/userstats` принимает числовой Telegram ID или уже виденный ботом `@username`. В общих отчётах ID намеренно не печатается; для точного SQL-разбора он остаётся в таблицах `telegram_messages`, `telegram_user_profiles` и `telegram_chat_users`.

## Prompt

Основной prompt лежит в [prompts/first_comment.md](prompts/first_comment.md).
Короткий факт-чек/RAG для защиты от устаревших утверждений лежит в [prompts/tech_rag.md](prompts/tech_rag.md).

Важно: модель должна вернуть текст с плейсхолдером `{CHAT_LINK}`. Код сам заменяет его на HTML-ссылку и не даёт модели портить URL.
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

- перед генерацией бот достаёт последние 6 ответов из `llm_generations`;
- prompt просит не повторять их начало, глаголы CTA и общий рисунок фразы;
- это снижает повторы вроде `залетайте`, `заходите`, `сравним`, `обсудим`.

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
- `/userstats` дополнительно показывает первое и последнее увиденное ботом сообщение пользователя по `telegram_chat_users`.
- `Завлечение после коммента` считает среднее число некомандных сообщений после комментария бота за 5 и 30 минут, плюс среднее число уникальных людей за 30 минут.
- `Комменты бота` сортируются по обсуждению за 30 минут, прямым реплаям и реакциям. Текст очищается от HTML/AI-маркеров и обрезается до короткого превью.

Что важно помнить по данным:

- Старые сообщения частично добиты миграцией из `raw_json`, но старые реакции Telegram Bot API не отдаёт.
- Reaction events и reaction count updates будут нулевыми, пока Telegram не начнёт присылать такие апдейты боту.
- Join/leave и точные member-status события зависят от того, какие `chat_member` updates Telegram реально отдаёт боту. На старте бот дополнительно делает best-effort `getChatMember` по последним виденным пользователям.
- Автоматическая конверсия по отдельной invite-ссылке пока не считается; входы через конкретную ссылку можно будет выделить, когда Telegram начнёт отдавать invite link в member events.

## Custom Emoji

Список считанных premium/custom emoji:

- [docs/custom_emoji_stickers.tsv](docs/custom_emoji_stickers.tsv)
- [docs/custom_emoji_sheet.png](docs/custom_emoji_sheet.png)

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
- Если Ollama Cloud вернёт ошибку/subscription limit, задача может остаться без комментария до ручного вмешательства.
- Join-конверсия по отдельной invite-ссылке пока не считается автоматически.
- Админки пока нет; настройки меняются через `.env` и рестарт сервиса.
