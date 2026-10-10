# Production deployment

Этот runbook описывает выкладку NedoBot на vps-153 (hostname сервера —
vps-13176). Deploy выполняется из слитого main, а не из рабочей feature
ветки. Фактически запущенный commit фиксируется immutable tag
deploy-YYYY-MM-DD-scope.

## Последняя фактическая выкладка

10 октября 2026 в **18:23 UTC / 21:23 МСК** выпущен release
[`deploy-2026-10-10-moderation-message-delete`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-10-moderation-message-delete)
на merge commit `7ea41fe8dab67dd1ffc43213f0f86e9611909425` после review
[PR #66](https://github.com/Mar2ianen/NedoBot/pull/66). [Release workflow 38074803057](https://github.com/Mar2ianen/NedoBot/actions/runs/38074803057)
успешно установил artifact в оба community-инстанса и MCP. SHA-256 обоих bot binaries —
`53aef7c8bccb6a6b8972389ef8faa2b84aa864676a4b1fab63b001ab5cf84a62`; SHA-256 MCP —
`2357ceb9d9b7460aced6b35516e4ad1221abe8591466e81fa4bea69c6451cfab`.

Все три unit-а active, `NRestarts=0`; хеши обоих работающих bot binaries совпали с artifact,
свежих error-level записей после рестарта нет. В PVO-профиле для `chats.review` включены
`moderation_delete_command_message=true` и `moderation_delete_target_message=true`;
в Nedonews эти опции не включены. Удаление выполняется только после успешного `/ban`, `/mute`
или `/warn`. Перед правкой PVO-профиля сохранена копия
`/etc/tg-ai-bot/pvo-llm_profiles.toml.bak-20261010T181144Z`; startup preflight подтвердил
право PVO-бота удалять сообщения.

10 октября 2026 в **11:55 UTC / 14:55 МСК** выпущен release
[`deploy-2026-10-10-pvo-risk-captcha`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-10-pvo-risk-captcha)
на merge commit `f33cfd4b9695b2205f5f06173f47f2c87a24d22d` после review [PR #63](https://github.com/Mar2ianen/NedoBot/pull/63). [Release workflow 38049697137](https://github.com/Mar2ianen/NedoBot/actions/runs/38049697137)
собрал artifact на Ubuntu 24.04, проверил glibc ≤ 2.39 и установил его в оба bot instance и MCP.
SHA-256 обоих bot binaries —
`248965b10992ad10dc9ebab5c2484eddce54a21cc19afc05364da847aff3b7f7`, MCP —
`0a6c0a8f008fc2878a59d094f4801a3af09447d1eedf002acd5b2a259dc5b186`.

Оба бота и `nedonews-mcp` active с `NRestarts=0`; running binary hashes совпали с artifact,
error-level journal после выкладки пуст. Обе production БД применили migrations
`20261010120000` и `20261010130000`; таблица `telegram_risk_captcha_challenges` есть в PVO.
При релизе в PVO-профиле включены `captcha_enabled=true`, `captcha_dry_run=false`, порог `70`, TTL `600` секунд;
предрелизная копия: `/etc/tg-ai-bot/pvo-llm_profiles.toml.bak-20261010T1148Z`. Startup preflight подтвердил права бота ПВО
на удаление сообщений и ограничения участников.
10 октября в **12:09 UTC / 15:09 МСК** runtime-порог капчи поднят с `70` до `85`; TTL остался 600 секунд.
Перед правкой сохранена копия `/etc/tg-ai-bot/pvo-llm_profiles.toml.bak-20261010T120842Z`, перезапущен только `nedobot-pvo`;
после рестарта все три unit-а active, `NRestarts=0`, свежих error-level записей нет.

9 октября 2026 в **20:21 UTC / 23:21 МСК** выпущен release
[`deploy-2026-10-09-userstatus-parallel`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-09-userstatus-parallel)
на merge commit `f6d1c00cfad8a081a58344a037a3974cf960cac4` после review
[PR #57](https://github.com/Mar2ianen/NedoBot/pull/57). Release workflow
[37985398485](https://github.com/Mar2ianen/NedoBot/actions/runs/37985398485)
успешно собрал artifact, проверил glibc ≤ 2.39 и установил его в оба bot
instance и MCP. SHA-256 обоих bot binaries —
`2ca45b0161e9b98b78e2832d71e2f73dc48312a5255c27d940e06d6cffe1cdec`, MCP —
`939fc9160824752b6000254432df717abf9f37831b1ed5f789bdc1ee0a378305`.

Оба бота и `nedonews-mcp` активны с `NRestarts=0`; хеши работающих процессов
совпали с release artifact, error-level journal после выкладки пуст. В
`/userstatus` независимое обновление member/profile и SQL-чтения выполняются
параллельно; до четырёх DB-запросов одновременно. На момент проверки в
20:24 UTC новых полных командных trace-ов после выкладки ещё не было, команду
smoke в чаты не отправляли.

8 октября 2026 в **19:20 UTC / 22:20 МСК** выпущен release
[`deploy-2026-10-08-telegram-send-fallback`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-08-telegram-send-fallback)
на merge commit `3b427de3e3caa3cc7a996b12ce8132b603c2d6c9` после review
[PR #47](https://github.com/Mar2ianen/NedoBot/pull/47). Release workflow
[37829976468](https://github.com/Mar2ianen/NedoBot/actions/runs/37829976468)
успешно собрал artifact, проверил glibc ≤ 2.39 и установил его в оба bot
instance и MCP. SHA-256 обоих bot binaries —
`48f05e673ebb67af0655804ce193644f2e172052468d8b6cb9649b480f3c65a2`, MCP —
`9bf767116cf9e3edf4b0e8d6229882ebf5861e57426d36967bd9ea8deac92bf2`.

В обеих production БД успешно применены все 94 миграции, failed migrations
нет; последняя — `20261008183924_preserve_spammer_avatar_dataset_refs`, колонка
`dataset_avatar_file_id` присутствует. Оба бота и `nedonews-mcp` активны с
`NRestarts=0`; hashes работающих bot processes и MCP совпали с artifact.
Error-level journal обоих ботов после выкладки пуст. Telegram API подтверждает
`can_read_all_group_messages=true` для Недострая и ПВО: обычные сообщения групп
читаются. Недострай не администратор группы (`restricted`), поэтому Telegram не
передаёт ему реакции и `chat_member`; его доступ к source channel при этом
подтверждён. ПВО — администратор. Команду-smoke в чаты не отправляли.

В релиз также вошёл `provider-access` как самостоятельный workspace crate;
приложение пока не использует его как runtime route.

8 октября 2026 в **16:26 UTC / 19:26 МСК** выпущен release
[`deploy-2026-10-08-ask-python-files`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-08-ask-python-files)
на merge commit `e1a6ab47b88f3eccfa2100c8857a8cbd544ce0f3` после review
[PR #45](https://github.com/Mar2ianen/NedoBot/pull/45). Сборка и deploy обоих
ботов и public MCP прошли в [release workflow 37807666497](https://github.com/Mar2ianen/NedoBot/actions/runs/37807666497).
Для обоих ботов SHA-256 исполняемого файла —
`5eef9f12104a6c0be1653effe42a039ece06fb3ef21d80c4c55eb9ef746a44cd`, для MCP —
`a9e4a1c16769431c5569b885dc201a13f45bc8426e981bd9db1d17beb9e4261c`.

В production включён `sandbox.python` только в профиле НедоNews; профиль ПВО
оставлен выключенным. Образ Python закреплён digest
`sha256:2a890751d3ac217ba36aab6235e15fa29d0f0e28f72ab040afac0d8180a0fdbd`.
Команда запускает одноразовый rootless Podman-контейнер от `nedobot-sandbox`:
без сети, host mounts, секретов и capabilities, с read-only root, лимитами CPU,
памяти, процессов и времени. Модель может передать ему один UTF-8 текстовый
документ из reply размером до 2 MiB; работают только стандартная библиотека
Python и временный `/workspace`. Файлы результата и состояние между вызовами
не сохраняются, постоянного Jupyter kernel нет. Startup preflight выполнил
`pass` в контейнере. До рестарта сделан закрытый backup основного профиля в
`/opt/tg-ai-bot-releases/deploy-2026-10-08-ask-python-files-before-20261008T162503Z/`.

После включения профиля оба бота и MCP перезапущены явным systemd system
manager; все три unit-а активны, `NRestarts=0`, application journal без ошибок.
Telegram smoke-команду не отправляли.

### Предыдущие выпуски 8 октября

PR [#44](https://github.com/Mar2ianen/NedoBot/pull/44), tag
[`deploy-2026-10-08-userstatus-rich-photo`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-08-userstatus-rich-photo),
устранил отказ `/userstatus`: renderer отдавал ссылку на кэшированный avatar
без передачи media, из-за чего Telegram отклонял всю rich-карточку. Workflow
[37800916143](https://github.com/Mar2ianen/NedoBot/actions/runs/37800916143)
успешно установил artifact `4266e6c8f8a3c6ded1f95800e3f6b98156e2fdcfda303b2bd3dc170c2e7a2997`.
Неполученные во время сбоя старые ответы Telegram не переигрывает; команду нужно
повторить.

PR [#43](https://github.com/Mar2ianen/NedoBot/pull/43), tag
[`deploy-2026-10-08-ask-context-grounding`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-08-ask-context-grounding),
обновил изоляцию контекста `/ask`, границы недоверенных данных и проверку
grounding цитат. Workflow
[37799237640](https://github.com/Mar2ianen/NedoBot/actions/runs/37799237640)
успешно завершился. Public MCP остаётся намеренно публичным read-only allowlist;
добавлять к нему Bearer-auth не требуется.

### Предыдущая выкладка: EmbeddingGemma 2, 7 октября

7 октября 2026 в **19:53 UTC / 22:53 МСК** выпущен release
[`deploy-2026-10-07-embeddinggemma2`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-07-embeddinggemma2)
на source `7883f750e86f333fba9f6da2e994dfc6bce73f48` после review
[PR #40](https://github.com/Mar2ianen/NedoBot/pull/40). Release workflow
[37676399623](https://github.com/Mar2ianen/NedoBot/actions/runs/37676399623)
собрал artifact и успешно установил его в оба checkout. В первой попытке
deploy шаг применил MCP `GRANT` раньше, чем startup-миграции обоих процессов
завершились; повторный запуск failed job прошёл после создания схемы. В этом
изменении workflow ждёт таблицы в обеих БД перед выдачей прав.

Оба бота и MCP запущены с artifact hashes:
bot `9401a65d3467da5cd513689fa74c3fa814091c873d9c19c3984abe223ed8aacf`,
MCP `00bfebb787904d1a3fcf68d6f10e8e0db82b556bfd7335e5c370a76ff721c317`.
В каждой БД 93 успешные миграции, failed migrations нет; все три unit-а имеют
`NRestarts=0`. Local MCP ответил `403`, public MCP — `405` на unauthenticated
GET; это ожидаемый ответ для этих probes. Оба production profiles используют
pinned `onnx-community/embeddinggemma-2-ONNX` Q4 revision
`daa72c51243991dfcaf9f9137d2c573d8f7790c0`, 512d text/image vectors и timeout
60 секунд. Main использует новый 512d spam-head; общая модерация ПВО осталась
выключенной. Новый `/embed` и `/embed-image` прошли smoke на тексте и
синтетическом PNG: по 512 конечных компонентов, L2 norm `1.0`.

`nedobot-rag-embedding` обслуживает новый encoder на `127.0.0.1:8788`.
Старый 300M chat unit остановлен и отключён, его unit и volume сохранены;
предыдущий RuBERT unit сохранён в snapshot. Для каждого инстанса запущены
chat/history backfill и avatar worker. Avatar dataset принимает только
векторы после явной spam-метки, удаляет строку после снятия метки и не хранит
image bytes. В момент старта workers нашли 177 ранее помеченных spammer-аватаров
с доступными Telegram file IDs; обработка и повтор ограниченных download errors
продолжаются.

До миграции сохранены оба PostgreSQL dump-а, consistent SQLite snapshot,
profiles, MCP env, units и работающие binaries в приватном
`/opt/tg-ai-bot-releases/deploy-2026-10-07-embeddinggemma2-before-20261007T192500Z/`.
Для обоих dump-ов проверены `pg_restore --list` и SHA-256; SQLite прошла
`PRAGMA integrity_check`.

### Предыдущая выкладка: changelog 6 октября

6 октября 2026 в **16:09 UTC / 19:09 МСК** выпущен release
[`deploy-2026-10-06-bot-changelog-details`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-06-bot-changelog-details)
на source `6949a151fc8e9ffd67419a8bb477ebd2b2077fc2`, после review
[PR #38](https://github.com/Mar2ianen/NedoBot/pull/38). Бинарники собраны
[release workflow](https://github.com/Mar2ianen/NedoBot/actions/runs/37492109948)
с `deploy=false`; `BUILD_INFO.txt` и все записи `SHA256SUMS` проверены перед
установкой. Обновлены и перезапущены оба bot-инстанса и `nedonews-mcp`.

В changelog добавлены антиспам-факты из коммитов 5 октября, вошедших в релиз
6 октября: сохранённые spam/not-spam метки учитываются при повторе имён,
заголовок личного канала служит сигналом, а доставка review-карточек (если
включена) ограничена первыми пятью минутами. Сейчас review delivery выключена
в обоих production profiles. Runtime profiles, env и схема БД не менялись.

Оба бота и MCP активны; `/proc/<pid>/exe` hashes совпали с артефактом:
bot `d3d910bc5a356fae9771fba0eabd28d20003d901f901bd6fac0ff868d0d636fe`,
MCP `26968db01032cf3f46508e2c3ece07906f0a0cbc2f232d02bf7199762a6140df`.
В каждой production БД 92 успешные миграции, failed migrations и ошибки
приложения после рестарта отсутствуют. Local MCP ответил `403`, public MCP —
`405` на unauthenticated GET; Telegram smoke-команды не отправлялись.

До выкладки сохранены и проверены dumps обеих БД (`pg_restore --list`),
consistent SQLite backup (`PRAGMA integrity_check`), работающие binaries,
profiles и env в приватном `/opt/tg-ai-bot-releases/deploy-2026-10-06-bot-changelog-details-before-20261006T155329Z/`.
Проверенный artifact и `DEPLOYMENT_RESULT.json` находятся в
`/opt/tg-ai-bot-releases/deploy-2026-10-06-bot-changelog-details-6949a15/`.

## Перед выкладкой

### Предыдущая выкладка changelog 6 октября, 18:36 МСК

Первая версия `/ask` changelog была выпущена tag
[`deploy-2026-10-06-bot-changelog`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-06-bot-changelog)
на source `6c11dc761872ae6ee7b650fb59fac2e2f7072f46` после review
[PR #36](https://github.com/Mar2ianen/NedoBot/pull/36). Её заменил текущий
release после повторной сверки заметных изменений из коммитов 5 октября.
Предыдущие stage и backup сохранены рядом с текущими в `/opt/tg-ai-bot-releases/`.

### Расширение лимитов /ask от 6 октября, 10:18 МСК

Фактическая выкладка: tag `deploy-2026-10-06-ask-limits`, source
`19f9ec21d091d59d935b38d727373911b5ea28ca`, review [PR #34](https://github.com/Mar2ianen/NedoBot/pull/34).
Точные artifact hashes и backup paths записаны в `TECHNICAL.md`.

В profile НедоNews после бэкапа вручную расширены шесть runtime-параметров:
16 384 токена, 64 tools / до 68 LLM turns, LLM-попытка 180 секунд,
общий deadline 1800 секунд, concurrency 4, MCP request 30 секунд.
Основной `groq_qwen_ask` использует `level_high` и timeout 180 секунд.
Добавлены отдельные `openrouter_qwen_ask` и `gemini_ask` с теми же effort/timeout;
`routes.ask.models` ссылается на них. Profile целиком из example не копировать.

`probe_ask_route [--image]` теперь валидирует startup secrets и проверяет
фактический `ask_llm_max_tokens`, печатая также tool budget и общий deadline.
До рестарта артефактом проверены staged Groq text/image, OpenRouter text
и Gemini image; после рестарта повторены Groq text/image на live profile.
Замена profile выполнена атомарно с проверкой исходного hash и сохранением
owner/mode. Новых миграций нет. Второй community profile сохранён без
изменений, `/ask` в ПВО выключен. Перезапуск и health check охватили
оба бота и MCP. Public RMCP проверен без отправки сообщений в Telegram.

### Релиз исправлений /ask от 6 октября, 09:30 МСК

Ниже сохранён порядок предыдущей выкладки исправлений и backfill.

Аудит и границы проверки: [ASK_AUDIT_2026-10-06.md](ASK_AUDIT_2026-10-06.md).
Сначала review `dev → main`, затем сборка release с `deploy=false` на точном
SHA и проверка `BUILD_INFO.txt`/`SHA256SUMS`. Деплой обоих ботов и MCP остаётся
отдельным действием.

В production profile нужна одна ручная правка после бэкапа: добавить
существующий `gemini_flash_comment` в конец `routes.ask.models`. Это
независимый vision fallback: текстовые OpenRouter/Ollama profiles при
приложенной фотографии исключаются capability-фильтром. Секрет
`GEMINI_API_KEY` уже используется текущими Gemini routes; profile целиком
из example не копировать. Второй community profile проверяется отдельно.

До рестарта можно проверить staged release CLI из каталога инстанса:

```bash
./probe_ask_route
./probe_ask_route --image
```

CLI загружает выбранный через `LLM_PROFILES_PATH` profile и secrets env;
вызывается только диагностический LLM tool, без Telegram, БД и заметок.
Для проверки ещё не изменённой production topology использовать отдельную
копию profile с добавленным fallback. Возвращаются identity выбранного
profile и `native_tool_verified`, содержимое ответа не печатается.

Проверка SDK артефактом с VPS подтвердила native tool calls Groq для текста
и изображения. Scoped dry run НедоNews обнаружил 52 восстанавливаемых
rich-сообщения. В ПВО найдены два rich-сообщения из Desktop export: их формат
тоже поддерживают backfill и обновлённый `import_telegram_export`.

При старте встроенная миграция `20261006000000` переочередит materialization
успешных v7 assessments в v8. LLM-аудит не повторяется. Контролировать
очередь и CAS/retry метрики обоих инстансов.

После миграций восстановить старые rich-тексты отдельно в каждой БД.
Выполнять из каталога инстанса с его env; CLI не требует Telegram token:

```bash
./target/release/backfill_rich_messages -1001932061163
./target/release/backfill_rich_messages -1001932061163 --apply
```

Для второго инстанса подставить его chat ID и DSN. Сначала проверить dry
run, затем `--apply`; повторный запуск идемпотентен. Payload и старый audit
не меняются, user activity не увеличивается, новые Telegram сообщения
и LLM jobs не создаются. Бэкап БД перед выкладкой обязателен.

Smoke после выкладки: reply на старый rich-ответ, вопрос о родительском
посте, точный word count с повтором слова в одной строке, групповой scope
при включённом private default chat, оба service PID/hash и journal.
Deployment tag ставить только после фактического успешного deploy.

1. Проверить, что PR слит в main, worktree чистый, а remote head известен:

   ```bash
   git fetch origin main
   git status --short --branch
   git rev-parse origin/main
   ```

2. Прогнать локальные проверки:

   ```bash
   cargo fmt --all -- --check
   cargo test --workspace --all-targets --locked
   cargo clippy --workspace --all-targets --locked -- -D warnings
   ./scripts/test.sh
   ```

3. Проверить production profile. В репозитории хранится только
   config/llm_profiles.toml.production.example: он не содержит секретов, но
   содержит реальные enabled flags, routes и команды MCP. На сервере он
   устанавливается в /etc/tg-ai-bot/llm_profiles.toml. Секреты остаются в
   /opt/tg-ai-bot-teloxide/.env и в systemd drop-ins.

   ```bash
   ssh vps-153 'test -f /etc/tg-ai-bot/llm_profiles.toml'
   ssh vps-153 'systemctl show tg-ai-bot-teloxide -p EnvironmentFiles'
   ssh vps-153 'systemctl cat tg-ai-bot-teloxide | sed -n "/llm-profiles.conf/,+3p"'
   ```

   Значение LLM_PROFILES_PATH должно быть абсолютным:
   /etc/tg-ai-bot/llm_profiles.toml.

## Инстансы

На vps-153 два community-инстанса одного бинаря, у каждого своя БД, env и
профиль. Общий только SQLite спам-репутации (один путь, разные `instance.id`).

| Инстанс | Unit | Checkout / binary | Profile | Database |
|---|---|---|---|---|
| НедоNews (`nedonews`) | `tg-ai-bot-teloxide.service` | `/opt/tg-ai-bot-teloxide/target/release/` | `/etc/tg-ai-bot/llm_profiles.toml` | `tg_ai_bot` |
| ПВО (`pvo`) | `nedobot-pvo.service` | `/opt/nedobot-pvo/target/release/` | `/etc/tg-ai-bot/pvo-llm_profiles.toml` | `tg_ai_bot_pvo` |

Оба бинаря ставятся из одного артефакта (`scripts/install_release_binaries.sh`
пишет в оба checkout). Рестарт и проверка всегда охватывают оба юнита:
`systemctl restart tg-ai-bot-teloxide nedobot-pvo`. Профили правятся вручную с
бэкапом и автоматикой не перезаписываются; шаблона pvo-профиля в репозитории
нет. Миграции встроены в бинарь (`sqlx::migrate!`), поэтому рассинхрон чекаута
и БД (как остановка `nedobot-pvo` 2026-10-01 из-за отсутствующей в его сборке
миграции) лечится выкладкой свежего бинаря, а не правкой `_sqlx_migrations`.

## Выкладка

Основной путь — GitHub Actions (`release`, раннер `ubuntu-24.04`, glibc 2.39
совпадает с production): сборка `--locked --release` с default features
(единый бинарь на все инстансы), проверка glibc-совместимости, публикация
артефакта и опциональный deploy на vps-153 с атомарной заменой бинарей,
restart и сверкой хеша `/proc/<pid>/exe`. Сборка на сервере больше не
используется: на 4 ГБ RAM `rustc` убивает OOM-killer. Для deploy из CI нужны
секреты репозитория: `VPS_HOST`, `VPS_USER`, `VPS_SSH_KEY`, `VPS_KNOWN_HOSTS`.
Конфиг `/etc/tg-ai-bot/llm_profiles.toml` автоматика не трогает: он правится
вручную с бэкапом, шаблон — `config/llm_profiles.toml.production.example`.

Запасной путь при недоступности CI — сборка на сервере (нужен свободный swap,
иначе OOM), затем restart и те же проверки. Сначала сделать dry-run. Не
использовать `--delete`: production checkout может
содержать SQLx migration-файлы, уже применённые к БД, но отсутствующие в текущем
source snapshot. Удаление такого файла приведёт к `VersionMissing` при следующем
старте. Устаревшие исходники удалять только отдельной проверенной процедурой
после сверки `_sqlx_migrations` обеих production-БД. Секреты, persistent
static-файлы, backups, дампы и локальный build cache исключаются явно:

В production есть один известный legacy gap: применённая миграция
`20260927120000` недоступна с исходным checksum. `db::migrate` разрешает
пропустить только эту отсутствующую версию после сверки applied versions;
любая другая отсутствующая версия остаётся startup error. Не добавляй файл с
тем же номером и другим SQL и не меняй checksum в `_sqlx_migrations`.

```bash
rsync -azn \
  --exclude target \
  --exclude .git \
  --exclude '.env*' \
  --exclude static/ \
  --exclude backups/ \
  --exclude models/ \
  --exclude '*.dump' \
  --exclude docs/LOCAL_WORKFLOW.md \
  ./ vps-153:/opt/tg-ai-bot-teloxide/
```

После проверки списка изменений повторить команду без -n, сохранив
machine-specific `docs/LOCAL_WORKFLOW.md`:

```bash
rsync -az \
  --exclude target \
  --exclude .git \
  --exclude '.env*' \
  --exclude static/ \
  --exclude backups/ \
  --exclude models/ \
  --exclude '*.dump' \
  --exclude docs/LOCAL_WORKFLOW.md \
  ./ vps-153:/opt/tg-ai-bot-teloxide/
```

`models/` исключён намеренно: весовые JSON лежат только на сервере (плюс
снапшоты в `/opt/tg-ai-bot-releases/`), а `--delete` без этого exclude уже
удалял `alt_word_char_v2_2026-10-03.json` 2026-10-05 с crash-loop НедоNews до
восстановления из снапшота (хеш сверен с задокументированным в релизе).

При использовании запасного пути повторить source sync для
`/opt/nedobot-pvo/`, сохранив его собственные `.env`, profile и persistent
файлы. Затем отдельно проверить доступ сервисного пользователя к обоим
checkout:

```bash
ssh vps-153 'chmod 755 /opt/tg-ai-bot-teloxide && runuser -u tg-ai-bot -- test -x /opt/tg-ai-bot-teloxide'
ssh vps-153 'chmod 755 /opt/nedobot-pvo && runuser -u tg-ai-bot -- test -x /opt/nedobot-pvo'
```

Не выполнять рекурсивный `chmod`: MCP нужен только проход по каталогу и
доступ к release binary. Изменения non-secret profiles внести вручную после
бэкапа, сохранив настройки каждого инстанса. Example целиком поверх
существующего profile не копировать. До рестарта проверить оба файла:

```bash
ssh vps-153 'test -s /etc/tg-ai-bot/llm_profiles.toml && test -s /etc/tg-ai-bot/pvo-llm_profiles.toml'
```

В запасном пути собрать один release на сервере с production toolchain и
локальным cargo cache. Подготовить stage из бинарей и installer, как в
workflow `release`, затем установить его через `install_release_binaries.sh`
сразу в оба checkout. Простого rebuild первого инстанса недостаточно:

```bash
ssh vps-153 'cd /opt/tg-ai-bot-teloxide && /root/.cargo/bin/cargo build --locked --release'
# После установки проверенного stage через installer:
ssh vps-153 'systemctl restart tg-ai-bot-teloxide nedobot-pvo'
ssh vps-153 'systemctl is-active tg-ai-bot-teloxide nedobot-pvo'
ssh vps-153 'systemctl restart nedonews-mcp'
ssh vps-153 'systemctl is-active nedonews-mcp'
```

### Опциональная Python-песочница `/ask`

Перед включением создай отдельного `nedobot-sandbox` с домашним каталогом
`/var/lib/nedobot-sandbox`, выделенным диапазоном subordinate UID/GID и включённым
lingering через `loginctl enable-linger nedobot-sandbox`. Бот запускается от
root, но вызывает Podman через `runuser` с очищенным окружением от имени этого
пользователя. Проверь его `podman info --format
'{{.Host.Security.Rootless}}'` и cgroup v2 лимиты. Подготовь
`deploy/ask-sandbox/Containerfile` от проверенного OCI digest; после сборки
получи immutable image ID через `podman image inspect --format '{{.Id}}'
<tag>` и укажи его в `ask_python_sandbox_image`. Включай
`ask_python_sandbox_enabled=true` только после успешного smoke запуска от
`nedobot-sandbox`. Образ во время запроса не скачивается (`--pull=never`).

Песочница принимает один UTF-8 текстовый файл до 2 MiB из reply для анализа
табличных данных, обычного текста и исходного кода; документ копируется через
stdin во временный `/workspace`. Каждый tool call получает новый контейнер, поэтому
состояние Python не хранится между вызовами и это не постоянное Jupyter-ядро.
Файлы-результаты отдельно не доставляются. Вложение, сеть и файлы хоста не
монтируются; сеть контейнера отключена, rootfs read-only, RAM ограничена 256 MiB
без swap, CPU и PID имеют лимиты, временный диск ограничен. На втором
community-инстансе оставь функцию выключенной, пока отдельно не проверишь
rootless Podman и лимиты ресурсов.

Для pinned EmbeddingGemma 2 text+image encoder создать volume и установить unit
из checkout; unit собирает pinned Transformers.js Q4 image из
`deploy/rag-embedding` при старте и скачивает model snapshot в постоянный volume:

```bash
ssh vps-153 'podman volume create nedobot_rag_embedding'
ssh vps-153 'install -m 0644 /opt/tg-ai-bot-teloxide/deploy/rag-embedding/nedobot-rag-embedding.service /etc/systemd/system/nedobot-rag-embedding.service'
ssh vps-153 'systemctl daemon-reload && systemctl enable --now nedobot-rag-embedding && curl -fsS http://127.0.0.1:8788/healthz'
```

Миграции запускаются startup-кодом бота. После проверки `/healthz` выставить в
обоих profile-файлах один pinned model id и URL `http://127.0.0.1:8788` для
RAG и chat retrieval; конфиги менять отдельно с бэкапом. Старые RuBERT 312d и
EmbeddingGemma 300M 768d колонки остаются нетронутыми для rollback. Новые
колонки заполняются бинарями `backfill_post_history_embeddings`,
`backfill_audit_embeddings` и `backfill_chat_embeddings` для каждой базы.
Старый cosine порог не переносится. Новая Gemma 2 spam-голова — отдельный
512d артефакт; устанавливать только этот файл рядом с сохранёнными моделями,
не синхронизировать каталог `models/` целиком.

`avatar_embeddings_enabled=true` включает отдельный bounded worker. Он
добавляет аватар только после явного spam-label; штатный аудит остальных
пользователей не сохраняет их image vectors. Снятие метки удаляет записи этого
пользователя. До векторизации Postgres держит file id только для повторной
загрузки; после готового embedding он очищается, остаётся только
non-downloadable file-unique id. Байты изображения не сохраняются.

После переключения проверить `nedobot-rag-embedding` и остановить старые
`nedobot-chat-embedding`/RuBERT-serving units. При rollback восстановить оба
profile-файла и старые unit-ы; legacy-векторы остаются отдельными и не
перезаписываются.

Для синхронизации репутации спамеров все локальные инстансы должны работать на
одном сервере и использовать одинаковый абсолютный путь к SQLite-файлу, например
`/var/lib/nedobot/spam-reputation.sqlite`. Это локальная SQLite-база в WAL-режиме,
не сетевой диск. Создать каталог с правами только для сервисного пользователя
перед запуском обоих инстансов; в каждом профиле установить
`[spam_reputation] enabled = true` и тот же `sqlite_path`. У каждого инстанса
должен быть собственный стабильный `instance.id`. Текущие локальные подтверждённые
метки загружаются в общий журнал при старте, затем статусы синхронизируются раз в
30 секунд; снятие локальной метки создаёт отзыв решения. Общие метки используются
как отдельный сильный сигнал для ревью и для поиска повторно используемых имён,
но сами по себе не вызывают бан.

## Проверка после restart

```bash
ssh vps-153 'systemctl is-active tg-ai-bot-teloxide nedobot-pvo nedonews-mcp container-tg-ai-bot-postgres nedobot-rag-embedding'
ssh vps-153 'journalctl -u tg-ai-bot-teloxide -u nedobot-pvo -n 120 --no-pager'
ssh vps-153 'journalctl -u nedonews-mcp -n 80 --no-pager'
ssh vps-153 'podman ps'
ssh vps-153 'curl -sS -o /dev/null -w "local=%{http_code} %{time_total}\n" http://127.0.0.1:8787/mcp/nedonews/v2'
curl -sS -o /dev/null -w 'public=%{http_code} %{time_total}\n' https://nedobot.chickenkiller.com/mcp/nedonews/v2
```

Для unauthenticated probe `403` на локальном endpoint и `405` на публичном
GET могут быть нормальным результатом: health-check подтверждает, что route
доступен, а не что MCP-клиент уже выполнил POST discovery. Реальный smoke
должен использовать MCP client с текущим RMCP v2 контрактом и допустимым Host/Origin; application auth намеренно отсутствует у публичного read-only endpoint-а.

Для Telegram runtime smoke используется отдельный тестовый чат и команда
/ping, затем /ask с коротким вопросом. Для /ask проверить:

- private chat получает native draft и один final answer;
- group получает один progress message, который редактируется до final;
- при rich delivery failure fallback не создаёт второе сообщение при
  Unknown;
- новые записи ask_runs содержат captured_now, timezone, renderer revision,
  compiled Markdown и delivery outcome.

## Фиксация release и rollback

Только после успешного restart и smoke создать и запушить annotated tag:

```bash
git tag -a deploy-YYYY-MM-DD-scope <merged-main-sha> \
  -m "Deploy NedoBot <merged-main-sha>"
git push origin deploy-YYYY-MM-DD-scope
```

Rollback — это повторная выкладка предыдущего release tag тем же способом:
checkout/archive exact tag, dry-run rsync, profile check, build, restart и
тот же post-deploy smoke. Не использовать git reset --hard на рабочем сервере
и не удалять .env, /etc/tg-ai-bot/llm_profiles.toml, static/, backups или
database dumps.
