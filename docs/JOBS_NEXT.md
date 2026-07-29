# Следующий этап: унификация lifecycle фоновых jobs

## Цель

Закрыть оставшийся пункт A4 аудита: распространить lease/CAS/bounded-retry инварианты на первый комментарий, embeddings и post history, не создавая универсальный SQL job repository.

Общий `features::jobs` предоставляет только policy-примитивы (`CasResult`, lease, poll, retry schedule). Claim SQL, eligibility, payload, статусы и side effects остаются в предметных модулях.

## Ключевое решение для первого комментария

Telegram `sendMessage` не принимает idempotency key. Если процесс упал после принятия сообщения Telegram, но до записи `bot_comment_message_id`, автоматически определить факт доставки нельзя.

Выбранная безопасная политика:

- перед Telegram send job переходит из `processing` в fenced `sending`;
- переход в `sending` обновляет отдельный короткий delivery lease;
- подтверждённый Telegram response финализируется транзакционно вместе с audit/history persistence;
- transport timeout/reset/invalid response после начала send и просроченный `sending` не отправляются автоматически повторно;
- такие случаи переходят в `delivery_unknown` и требуют reconciliation;
- приоритет — не создавать дублирующий первый комментарий.

Это at-most-once политика для неоднозначного crash window, а не обещание exactly-once доставки.

## Task JOB-1 — Durable first-comment delivery phase — выполнено, ожидает deploy

**Приоритет:** P0 закрыт в коде; deployment требует отдельной production-проверки дублей `llm_generations.post_comment_job_id`.

**Файлы:**

- `src/features/first_comment/repo.rs`
- `src/features/first_comment/pipeline.rs`
- `src/features/jobs/claim.rs`
- `src/features/jobs/policy.rs`
- `src/main.rs`
- новая additive migration
- `tests/postgres_migrations.rs` либо отдельный PostgreSQL integration test

**Изменения:**

- расширить единственный `status` state machine: `pending`, `retry_wait`, `processing`, `sending`, `sent`, `failed`, `delivery_unknown`; не добавлять ортогональную `delivery_phase`;
- добавить/сохранить timestamps `processing_started_at`, `sending_started_at`, `lease_expires_at`;
- fenced transition `processing -> sending` выполнять до внешнего вызова и продлевать lease до `now() + POST_COMMENT_DELIVERY_LEASE`;
- expired `sending` переводить в `delivery_unknown`, не в обычный retry;
- разделить domain finalizers: `mark_post_comment_pre_send_failed`, `begin_post_comment_delivery`, `mark_post_comment_send_rejected`, `mark_post_comment_delivery_unknown`, `finalize_post_comment_sent`;
- все finalizers возвращают `CasResult`; lease/attempt guard сохраняется во всех переходах;
- после подтверждённого Telegram send одной DB-транзакцией начать с fenced `sending -> sent` CAS update, затем:
  - сохранить `bot_comment_message_id` и `sent_at`;
  - вставить `llm_generations` идемпотентно;
  - enqueue post-history job идемпотентно;
  - commit;
- если любой DB statement после Telegram response упал, transaction откатывается и job остаётся `sending`: по expiry она станет `delivery_unknown`, а не причиной повторного Telegram send;
- owner preview остаётся post-commit best-effort;
- LLM/download/render errors до `sending` используют текущий bounded schedule `15s -> 60s -> failed`;
- подтверждённые Telegram rejection errors из `sending` используют bounded retry или terminal `failed` по error kind;
- network timeout/reset/invalid transport response после fenced send переходят в `delivery_unknown`;
- `RetryAfter` считается подтверждённым rejection и задаёт нижнюю границу задержки.

**Migration policy:**

- только additive migration;
- добавить schema-level idempotency для `llm_generations(post_comment_job_id)` после отдельной production-проверки существующих дублей; migration не удаляет дубли автоматически;
- старые `sent/failed/pending/retry_wait` сохраняют смысл;
- любая legacy row со `status = 'processing'` мигрируется в `delivery_unknown`, независимо от lease: legacy schema не может доказать, был ли уже принят Telegram send;
- migration никогда автоматически не переотправляет legacy `processing` row;
- добавить CHECK/index для новых claimable и ambiguous states;
- не изменять уже применённые migrations.

**Acceptance criteria:**

- stale attempt не может перевести новую попытку в `sent/retry_wait/failed`;
- failure audit/history persistence после Telegram response не ставит внешний send на повтор;
- expired ambiguous send становится `delivery_unknown`;
- normal successful path создаёт ровно одну generation и одну history job;
- `retry_pending_comments` не выбирает `delivery_unknown` для автоматической отправки;
- документация честно описывает at-most-once ambiguous policy и distinct processing/delivery leases.

**Обязательные PostgreSQL tests:**

1. legacy `processing -> delivery_unknown` migration и отсутствие automatic claim;
2. claim #1 -> lease expiry -> claim #2 -> finalization #1 = `LeaseLost`;
3. `processing -> sending` требует текущий attempt и обновляет delivery lease;
4. expired `sending -> delivery_unknown`, normal worker и `retry_pending_comments` его не выбирают;
5. transport error после fenced send -> `delivery_unknown`; `RetryAfter` -> `retry_wait` с нижней границей задержки;
6. confirmed send транзакционно сохраняет job + generation + history enqueue;
7. идемпотентный generation/history conflict не создаёт дублей и позволяет `sent`;
8. реальная DB-ошибка откатывает transaction: job не становится `sent` и не вызывает повторный Telegram send;
9. migration сохраняет поведение legacy `pending/retry_wait/sent/failed`.

## Task JOB-2 — Fence embedding batch finalization — выполнено, ожидает deploy

**Приоритет:** P0 закрыт в коде; выполнен независимо от JOB-1.

**Файлы:**

- `src/features/chat_retrieval.rs`
- `src/features/jobs/claim.rs`
- `src/features/jobs/policy.rs`
- PostgreSQL integration tests

**Изменения:**

- `mark_embedding_ready/failed` проверяют `attempts = claimed_attempts`;
- affected rows преобразуются в `CasResult`;
- stale worker логирует lease loss и не меняет новую попытку;
- до finalization проверить `embeddings.len() == jobs.len()`;
- short/oversized provider batch считается общей retryable batch failure, а не оставляет хвост jobs в `processing`;
- добавить именованные `CHAT_EMBEDDING_LEASE` и `CHAT_EMBEDDING_RETRY`, сохранив текущие интервалы.

**Migration:** не требуется.

**Acceptance criteria:**

- stale success/failure возвращает `LeaseLost`;
- partial provider response переводит все ещё принадлежащие worker jobs в bounded retry;
- eligibility re-check для edited/deleted/spam/bot/automatic-forward сообщений остаётся intact.

## Task JOB-3 — Explicit lease для post history — выполнено, ожидает deploy

**Приоритет:** P1 закрыт в коде; additive migration безопасно backfill-ит legacy processing lease.

**Файлы:**

- `src/features/memory/service.rs`
- `src/features/jobs/claim.rs`
- `src/features/jobs/policy.rs`
- `src/main.rs`
- новая additive migration
- PostgreSQL integration tests

**Изменения:**

- добавить `lease_expires_at` в `post_history_entries`;
- заменить implicit reclaim через `processing_started_at < now() - 5 minutes` на explicit lease;
- claim сделать единым `CTE UPDATE ... RETURNING` с `SKIP LOCKED`;
- success/retry/fail finalizers возвращают `CasResult`;
- все выходы очищают lease;
- вынести именованные `POST_HISTORY_LEASE`, `POST_HISTORY_RETRY`, `POST_HISTORY_WORKER_POLL`;
- сохранить текущую геометрическую retry policy и terminal boundary после десятой попытки.

**Acceptance criteria:**

- параллельные claims получают разные rows;
- expired lease reclaim увеличивает attempt;
- stale attempt не может save/retry/fail;
- disabled RAG не claim-ит работу и не создаёт внешний запрос;
- processing lease имеет partial index.

## Task JOB-4 — Lifecycle observability и reconciliation — выполнено

**Приоритет:** P2, после появления `delivery_unknown`.

**Файлы:**

- domain repositories трёх очередей;
- `src/bin/retry_pending_comments.rs`;
- при необходимости новый read-only/reconcile CLI;
- `docs/TECHNICAL.md`.

**Метрики/операционные данные:**

- возраст старейшей ready job;
- ready/processing/retry/failed/delivery_unknown counts;
- attempts и terminal failures по безопасному `error_kind`;
- lease reclaim count;
- embedding batch-cardinality failures.

**Ограничения:**

- не логировать raw API bodies, токены и Telegram URLs с token;
- reconciliation по умолчанию read-only;
- повторная отправка `delivery_unknown` только отдельной явной operator-командой.

**Реализация:** `reconcile_comment_delivery` не запускает миграции. `list` и `inspect` только читают БД; `mark-delivered` и `mark-failed` делают только fenced DB-переход из `delivery_unknown` и пишут append-only audit. `retry` требует `--acknowledge-duplicate-risk`, atomically claim-ит ровно указанную ambiguous row с `operator_retry_only`, и только затем создаёт `Config`/`Bot` и запускает реальный pipeline. Обычный worker и `retry_pending_comments` не claim-ят такую row. Ошибка operator retry до send или подтверждённый Telegram rejection становится terminal `failed`; транспортная неоднозначность остаётся `delivery_unknown`.

## Task JOB-5 — Остаточные regression tests и индексы

**Приоритет:** P2, выполнять вместе с соответствующими domain tasks.

- прямой DB-test разделения `claim sequence` и retry ordinal для review delivery;
- success после нескольких review failures сбрасывает consecutive failures;
- migration fixture для legacy first-comment `processing -> delivery_unknown`;
- второй partial index spam-review по expired processing leases добавить только при подтверждённом росте таблицы/slow query;
- EXPLAIN/queue-size check перед добавлением индекса;
- test fixtures для upgrade-path каждой новой lifecycle migration.

## Порядок реализации

1. JOB-1: first-comment delivery fencing и `delivery_unknown` — выполнено, ожидает deploy после проверки production-дублей generation.
2. JOB-2: embedding attempt-CAS и batch cardinality — выполнено, ожидает deploy.
3. JOB-3: post-history explicit lease — выполнено, ожидает deploy.
4. JOB-4: observability/reconciliation — выполнено.
5. JOB-5: остаточные regression/performance задачи.

JOB-1 и JOB-2 имеют непересекающиеся domain write sets и могут разрабатываться параллельно, но migration и итоговую документацию следует коммитить отдельными логическими единицами.

## Проверки для каждого нетривиального task

```bash
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
./scripts/test.sh
```

Изменения migrations, claim/finalization и public job projections обязательно получают PostgreSQL integration tests.
