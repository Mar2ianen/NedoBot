# Production deployment

Этот runbook описывает выкладку NedoBot на vps-153 (hostname сервера —
vps-13176). Deploy выполняется из слитого main, а не из рабочей feature
ветки. Фактически запущенный commit фиксируется immutable tag
deploy-YYYY-MM-DD-scope.

## Перед выкладкой

### Релиз исправлений /ask от 6 октября

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
   cargo fmt -- --check
   cargo test --all-targets
   cargo clippy --all-targets -- -D warnings
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

После rsync отдельно проверить доступ сервисного пользователя к checkout:

```bash
ssh vps-153 'chmod 755 /opt/tg-ai-bot-teloxide && runuser -u tg-ai-bot -- test -x /opt/tg-ai-bot-teloxide'
```

Не выполнять рекурсивный `chmod`: MCP нужен только проход по каталогу и
доступ к release binary. Затем установить non-secret profile отдельно и
проверить его наличие до рестарта:

```bash
rsync -az config/llm_profiles.toml.production.example \
  vps-153:/etc/tg-ai-bot/llm_profiles.toml
ssh vps-153 'test -s /etc/tg-ai-bot/llm_profiles.toml'
```

Сборка и restart выполняются на сервере, чтобы release binary использовал
production toolchain и локальный cargo cache:

```bash
ssh vps-153 'cd /opt/tg-ai-bot-teloxide && /root/.cargo/bin/cargo build --release'
ssh vps-153 'systemctl restart tg-ai-bot-teloxide'
ssh vps-153 'systemctl is-active tg-ai-bot-teloxide'
ssh vps-153 'systemctl restart nedonews-mcp'
ssh vps-153 'systemctl is-active nedonews-mcp'
```

Перед первым включением новой chat-semantic ветки один раз установить unit и
загрузить модель в persistent volume. Модель — GGUF-конвертация официальных
Google QAT-весов, а не файл, который должен попадать в checkout:

```bash
ssh vps-153 'install -m 0644 /opt/tg-ai-bot-teloxide/deploy/chat-embedding/nedobot-chat-embedding.service /etc/systemd/system/nedobot-chat-embedding.service && podman volume create nedobot_chat_embedding'
ssh vps-153 'podman run --rm -v nedobot_chat_embedding:/models docker.io/curlimages/curl:8.10.1 -fL -o /models/embeddinggemma-300M-qat-Q4_0.gguf https://huggingface.co/ggml-org/embeddinggemma-300M-qat-q4_0-GGUF/resolve/main/embeddinggemma-300M-qat-Q4_0.gguf'
ssh vps-153 'systemctl daemon-reload && systemctl enable --now nedobot-chat-embedding && curl -fsS http://127.0.0.1:8795/health'
```

После этого migrations запускаются startup-кодом бота. Затем убедиться, что в
journal нет ошибки profile validation или migration и что контейнер PostgreSQL
доступен. `nedobot-rag-embedding` не пересобирается при обычной выкладке бота,
но его health нужно проверить, если включены RAG или другие старые memory/audit
потоки.

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
ssh vps-153 'systemctl is-active tg-ai-bot-teloxide nedobot-pvo nedonews-mcp container-tg-ai-bot-postgres nedobot-rag-embedding nedobot-chat-embedding'
ssh vps-153 'journalctl -u tg-ai-bot-teloxide -u nedobot-pvo -n 120 --no-pager'
ssh vps-153 'journalctl -u nedonews-mcp -n 80 --no-pager'
ssh vps-153 'podman ps'
ssh vps-153 'curl -sS -o /dev/null -w "local=%{http_code} %{time_total}\n" http://127.0.0.1:8787/mcp/nedonews/v2'
curl -sS -o /dev/null -w 'public=%{http_code} %{time_total}\n' https://nedobot.chickenkiller.com/mcp/nedonews/v2
```

Для unauthenticated probe `403` на локальном endpoint и `405` на публичном
GET могут быть нормальным результатом: health-check подтверждает, что route
доступен, а не что MCP-клиент уже выполнил POST discovery. Реальный smoke
должен использовать MCP client с разрешённым origin/auth контрактом.

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
