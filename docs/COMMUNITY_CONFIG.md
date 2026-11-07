# Community instance configuration

NedoBot runs one Telegram bot token as one community instance. A process may
own several managed chats in that community, but it does not contain a
multi-tenant registry of unrelated communities.

The LLM profile TOML must contain these non-secret sections:

```toml
[instance]
id = "spbvelo"
display_name = "Петербургское велообщество"
timezone = "Europe/Moscow"

[telegram]
unknown_chat_policy = "ignore"
owners = [5939287960]

[chats.general]
id = -1001111111111
ingest = true
moderation = true
stats = true
voice = false
ask = false
invite_url_env = "CHAT_INVITE_URL"
invite_label = "чате"

[chats.review]
id = -1004444444444
ingest = false
moderation = false
stats = false
voice = false
ask = false
review_destination = true

[moderation]
enabled = true
risk_profile = "ru_general_v1"
review_chat = "review"
reviewer_user_ids = [5939287960]
captcha_enabled = false
captcha_dry_run = true
captcha_threshold = 70
captcha_ttl_sec = 600
```

`captcha_enabled` включает challenge для новых участников, чей score не ниже
и `captcha_threshold`, и `review_threshold` выбранного risk-профиля.
Для проверки сохраняются текущие права чата, участник временно лишается всех
прав отправки, а первое сообщение удаляется. Только правильный ответ снимает
это ограничение и возвращает сохранённые права. Капча не снимается по таймеру;
после трёх неверных ответов пользователь остаётся ограничен до ручного решения.
Сначала оставьте `captcha_dry_run = true`, затем включайте реальные действия
только в профиле нужного instance. Капча заменяет автоудаление/автобан для
подходящих по score пользователей и не создаёт spam label.
TTL начинается после отправки challenge. По истечении 10 минут участник
обратимо удаляется из чата (kick без постоянного бана); при повторном входе
high-risk аудит может выдать новую капчу.
Для реальных действий бот должен быть администратором с правами
`can_delete_messages` и `can_restrict_members` в каждом модерируемом чате.

Spam-review buttons are available to configured `reviewer_user_ids` and to
current administrators/owners of the configured `review_chat`.

Chat invite configuration belongs to the chat entry, so `/ask` remains
independent from first-comment routes. `first_comment.routes[*].invite_url_env`
is retained as a per-route override for communities that use different public
links.

`[instance]`, `[telegram]` and at least one `[chats.*]` entry are required at
startup. Chat IDs must be unique. The bot ignores unknown group chats before
message, profile, reaction, member-event or audit persistence.

Commands and reports use the chat in which they were invoked as their default
scope. The response chat and the database scope are separate arguments in the
stats transport, so an admin-DM transport can be added without silently
changing the source chat.

`moderation.risk_profile` names a versioned policy. The profile can carry the
Telegram ID model metadata:

```toml
[risk_profiles.ru_general_v1]
version = "ru-general-v1"
old_user_message_threshold = 5
review_threshold = 70

[risk_profiles.ru_general_v1.telegram_id]
floor = 0.0066813
ceil = 1.0
k = 3.20435
midpoint_billion = 8.45919
version = "4pl-prod-2026-09-21"
```

Audit rows persist the selected profile and model versions. The configured
invite URL remains a secret/environment value; no Telegram invite or chat ID
is supplied by Rust defaults.

The current release profile keeps the existing Nedonews modules enabled by
default. A moderation-only community build can be checked with:

```bash
cargo check --no-default-features --features "moderation,spam-sync" --all-targets
```

If a config enables a module that is absent from the binary, startup fails
instead of silently ignoring that setting.

## MCP для отдельного community instance

Read-model scope берётся из authoritative community profile (`[instance].id`,
primary `[chats.*].id` и source-channel активного route), а не из аргументов
MCP-запроса. Каждому instance нужны отдельные PostgreSQL database, Telegram bot
token, профиль и manifest. Не переиспользуйте базу, пользовательскую память или
Telegram token между независимыми community instances. Числовые chat/channel IDs
не задаются в общем коде или manifest: они читаются из profile конкретного
instance.

Пока DB read-model охватывает только primary chat instance-а, `/ask` в другом
managed chat явно отклоняется до старта MCP retrieval. Ответы и сообщения бота
по-прежнему адресуются исходному чату; его ID не меняет доверенный read scope.

### Подготовка новой базы

1. Создайте новую базу и отдельную read-only роль MCP до применения миграций.
   Примените обычные миграции приложения, включая
   `20261107150000_instance_scoped_mcp_views.sql`. Если роль `nedobot_mcp_ro`
   уже существует в этой базе, миграции выдадут ей доступ только к reviewed
   `mcp_public` views, `mcp_private.telegram_media` и функции проверки scope;
   таблица `mcp_private.instance_scope` останется закрыта. Миграция сохраняет прежний
   NedoNews scope при upgrade уже существующей базы; она не угадывает scope новой
   базы. У новой базы должен быть собственный набор таблиц и reviewed views.
2. Создайте отдельный LLM/community profile (например, для `community_a`) с
   уникальным `[instance].id`, его discussion chat ID в primary `[chats.*]` и
   source channel ID в route при наличии. Добавьте Telegram token в секретное
   окружение этой установки; токены не помещайте в profile, manifest или
   репозиторий.
3. Один раз provision-ите scope от имени владельца этой базы. Перед сменой scope
   остановите все MCP процессы, использующие эту базу: каждый живой сервер держит
   shared advisory lease, а PostgreSQL отклоняет прямое изменение scope без
   exclusive lock provisioner-а. Команда сверяет конфигурацию и не выдаёт права
   на таблицу scope read-only MCP роли:

   ```bash
   DATABASE_URL="$INSTANCE_DATABASE_URL" \
   LLM_PROFILES_PATH=/etc/nedobot/community_a/llm_profiles.toml \
   cargo run --release --bin provision_mcp_instance_scope -- --confirm-scope-change
   ```

   На новой базе `--confirm-scope-change` обязателен после установки migration;
   для совпадающего scope команда безопасно завершится без изменений. После
   перепривязки запустите MCP снова с обновлёнными profile и manifest: старый
   процесс не даст выполнить provision, а новый сверит scope до публикации tools.
4. Сгенерируйте и проверьте отдельный manifest на этой же базе и с тем же
   профилем:

   ```bash
   DATABASE_URL="$INSTANCE_DATABASE_URL" \
   LLM_PROFILES_PATH=/etc/nedobot/community_a/llm_profiles.toml \
   cargo run --release --bin generate_mcp_db_manifest -- \
     /etc/nedobot/community_a/mcp_db_manifest.toml
   ```

   Manifest фиксирует scope и точные имена, типы и nullable-статус колонок.
   Не коммитьте generated manifest, если он содержит приватную схему или данные;
   сверяйте его с reviewed `mcp_public` контрактом.
5. Запускайте bot и MCP с собственными `LLM_PROFILES_PATH` и database URL.
   Internal stdio MCP получает `ASK_DATABASE_URL` и путь `MCP_MANIFEST` из
   allowlist окружения приложения, а instance scope — из профиля родительского
   бота, не из tool arguments. Для HTTP MCP задайте `ASK_DATABASE_URL`,
   `LLM_PROFILES_PATH`, `MCP_MANIFEST` и собственный статический `MCP_PATH`
   (например, `/mcp/community-a/v2`). Его read-only роль должна иметь доступ только
   к reviewed `mcp_public` views и нужным безопасным внутренним функциям; права
   на `public.*`, базовые таблицы и `mcp_private.instance_scope` не выдавайте.

HTTP/public MCP по умолчанию выключен. Для отдельного публичного endpoint его
можно включить только явным `[public_mcp] enabled = true` в профиле; HTTP
bootstrap проверит профиль и совпадение профиля, manifest и database scope.
Это отдельное решение о публикации содержимого сообщества, оно не включает
публичный endpoint автоматически при запуске internal stdio MCP.
