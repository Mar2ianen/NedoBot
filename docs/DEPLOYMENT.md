# Production deployment

Этот runbook описывает выкладку NedoBot на vps-153 (hostname сервера —
vps-13176). Deploy выполняется из слитого main, а не из рабочей feature
ветки. Фактически запущенный commit фиксируется immutable tag
deploy-YYYY-MM-DD-scope.

## Перед выкладкой

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

## Выкладка

Сначала сделать dry-run. --delete не должен затронуть секреты, persistent
static-файлы, backups, дампы и локальный build cache:

```bash
rsync -azn --delete \
  --exclude target \
  --exclude .git \
  --exclude '.env*' \
  --exclude static/ \
  --exclude backups/ \
  --exclude '*.dump' \
  --exclude docs/LOCAL_WORKFLOW.md \
  ./ vps-153:/opt/tg-ai-bot-teloxide/
```

После проверки списка изменений повторить команду без -n, сохранив
machine-specific `docs/LOCAL_WORKFLOW.md`:

```bash
rsync -az --delete \
  --exclude target \
  --exclude .git \
  --exclude '.env*' \
  --exclude static/ \
  --exclude backups/ \
  --exclude '*.dump' \
  --exclude docs/LOCAL_WORKFLOW.md \
  ./ vps-153:/opt/tg-ai-bot-teloxide/
```

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

## Проверка после restart

```bash
ssh vps-153 'systemctl is-active tg-ai-bot-teloxide nedonews-mcp container-tg-ai-bot-postgres nedobot-rag-embedding nedobot-chat-embedding'
ssh vps-153 'journalctl -u tg-ai-bot-teloxide -n 120 --no-pager'
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
