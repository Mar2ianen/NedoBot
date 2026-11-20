# Интеграционный аудит NedoBot PR #126–#173

Дата: 2026-11-19 (обновлено после первого ревью PR #174). База: `main` @ `7b8815bbff80c4665b64076a063f5f4919e5465a` (merge #126).
Интеграционная ветка: `codex/integration-audit-126-173`, PR #174.
Проверенный кодовый SHA: `38efa229830c01a93da5416c787d8c79aee5c4d6`.
CI на этом SHA: push run [44536297777](https://github.com/Mar2ianen/NedoBot/actions/runs/44536297777) — success, PR run [44536310348](https://github.com/Mar2ianen/NedoBot/actions/runs/44536310348) — success. Отчётный коммит меняет только этот файл и `testing/QUALITY_FINAL.md`, то есть не меняет проверенный код; его exact-head run виден в checks PR #174.

## 1. DAG и фактическая последовательность интеграции

Сначала были обновлены refs и зафиксированы головы PR. PR #126 уже был merged в `main`; #127–#132 независимо стоят на `main`; #133–#155 — цепочка refactor; #156 — отдельная база тестового пакета, #157–#173 — цепочка тестов поверх #129.

| Шаг | Что включено | Коммит интеграции | Голова включённого PR |
|---|---|---|---|
| 0 | База `main` после #126 | `7b8815bb` | #126 `a6571d2a` |
| 1 | Refactor-цепочка #133→#155 | `d1a6d36` (merge) | #155 `b817822a` |
| 2 | Testing-цепочка #156→#173 (включает #129) | `7906e2d` (merge) | #173 `f6f552ac` |
| 3 | #127 локальный alias resolver | `cc82a0e` (merge) | `6b24f04b` |
| 4 | #128 URL inventory | `155c491` | `80370ea3` |
| 5 | #130 two-chat URL history (переработан) | `a187404` | `d59f7c51` |
| 6 | #131 alias churn tests | `b0c77cf` | `91f2d8d9` |
| 7 | #132 entity boundary fix | `cc154b1` | `debc68e3` |
| 8 | #156 инвариантная матрица | `964c05d` | `a8278795` |
| 9 | Единая модель alias и сериализация | `426eef1` | — |
| 10 | P1/P2 исправления и quality lanes | `dfd7057`…`fad4710` | — |

Порядок выбран так, чтобы сначала получить компилируемое refactor-дерево, затем наложить тестовый стек, затем — независимые от `main` PR по одному. Слепое слияние двух стеков не применялось.

### Конфликты при совмещении #155 и #173

`git merge-tree d1a6d36 f6f552a` даёт 13 конфликтных файлов:

```
.github/workflows/ci.yml
scripts/test.sh
src/db/url_evidence.rs
src/features/ask/agent.rs
src/features/auto_moderation.rs
src/features/first_comment/pipeline.rs
src/features/first_comment/render.rs
src/features/memory/service.rs
src/main.rs
src/telegram/entities.rs
src/telegram/manual_moderation.rs
src/telegram/render.rs
tests/postgres_migrations.rs
```

Конфликты разрешены не выбором одной стороны: сохранены typed-модели refactor-стека, тестовые оракулы/фикстуры testing-стека и оба набора CI-проверок. Отдельно сведены `memory::classify_error`, router wiring, Telegram entity/delivery модели, pipeline-тесты и PG suite.

## 2. Реестр дефектов

| ID | Prio | PR/SHA | path:line | Статус | Минимальный контрпример / до → после | Тест | Mutation | Коммит | Остаточный риск |
|---|---|---|---|---|---|---|---|---|---|
| P1.1 | P1 | #170 `f702944` | `src/features/search/mcp.rs:427`, `:458`, `:491` | fixed для literal/URL-политики; DNS/redirect — unverified | `100.64.0.1`, `198.18.0.1`, `192.0.2.1`, `[2001:db8::1]`, `[::ffff:100.64.0.1]` принимались; `http://@example.com` терял пустой userinfo при нормализации. После — отвергаются; глобальные literal и `localhost.example.com` проходят | `fetch_url_filter_rejects_private_and_non_http_targets`, `ipv4_special_registry_ranges_reject_both_ends_and_accept_adjacent_addresses`, `ipv6_special_registry_boundaries_reject_non_global_and_accept_global_unicast`, `nat64_and_ipv4_mapped_hosts_reuse_ipv4_classification` | 2/2 caught | `b5e7a25` + `7906e2d` | Фактический fetch делегирован внешнему MCP; DNS rebinding и redirect hop не контролируются. Явная trust boundary, не «fixed» |
| P1.2 | P1 | #152/#168 | `src/features/memory/service.rs:598` | fixed | `http_503` классифицировался как `error`; вложенный `LlmTransportError::HttpStatus(429)` терялся; текст `"HTTP 429 timeout JSON"` мог управлять retry. После: typed `http_503`, `http_429`, `timeout`, `validation_failed`; spoof-текст → `error` | `error_classifier_preserves_all_typed_statuses_and_validation_causes_in_chains` | 4/4 caught | `e016a6e` + `7906e2d` | Сохранён persisted-формат `http_<status>`/`timeout`/`validation_failed`; исторические значения не переписываются |
| P1.3 | P1 | #139/#158 | `src/telegram/router/commands.rs:6`; `src/telegram/router/mod.rs:36` | fixed | Тесты макроса не доказывали production graph; корневой `filter_message()` мог поглощать не-message updates. После тесты вызывают `telegram::router::build(Routes { … })`; command идёт до ordinary message, edited/reaction/reaction_count/callback/member изолированы, unsupported — без side effects | `actual_router_builder_prefers_command_and_falls_back_for_unknown_command`, `…routes_edits…`, `…routes_reaction…`, `…routes_callback_updates_once`, `…has_no_side_effects…` | Ручной branch-order mutant убит production-тестом (`scripts/run_router_order_mutation.sh`) | `fad4710` + `cc82a0e` + `d1a6d36` | ACK ровно один раз проверяется на уровне callback adapter (`callbacks.rs`), не в dptree graph |
| P1.4 | P1 | #144 `727872a` | `src/features/jobs/worker.rs:159`, `:182`, `:242` | fixed | Worker завершался/паниковал, процесс продолжал жить. После return/panic/abort → typed `WorkerFailure` после bounded drain; graceful dispatcher shutdown — не fatal; drain timeout абортит и дропает зависшие задачи | 10 worker lifecycle tests, включая `unexpected_worker_{completion,panic,abort}_fails_foreground_supervision`, `completed_foreground_drains_workers_without_fatal_error`, `drain_timeout_aborts_and_drops_stuck_workers`, `record_exit_logs_graceful_and_unexpected_completion_differently` | 4 caught, 1 unviable (`record_exit` → `Some(Default::default())`) | `26dd88b` + `727872a` | systemd/restart policy в репозитории не проверялась и не заявляется |
| P1.5 | P1 | #171–#173 | `scripts/test.sh:7`; `.github/workflows/quality-nightly.yml:33` | fixed | `cargo test --no-default-features --workspace --all-targets --locked`: было 20 failed из-за production TOML/env и feature-gates; после — `392 passed, 0 failed, 4 ignored`. URL-тест больше не вызывает `Config::from_env()` | Полный no-default workspace suite; `fetch_urls_never_forwards_blocked_ip_literals_and_keeps_lookalike_hosts_distinct` на синтетическом config | — | `b5e7a25` + `314f26e` | 4 ignored — только PG-тесты, требующие disposable БД; они запускаются отдельными lanes |
| P1.6 | P1 | ревью #174 / `8fb7986` | `src/features/user_profiles/service.rs:102`, `:143`; `src/db/telegram.rs:703` | fixed | `getChat` ошибка + успешный `getUserProfilePhotos`: до — `username=NULL`, alias deactivated, `profile_refreshed_at=now()`; после — обновляются только photo-поля, username/first_name/bio/chat-photo/timestamp сохраняются, ошибка пишется в `profile_refresh_error`; успешный `getChat` без username по-прежнему очищает alias | `partial_profile_refresh_preserves_confirmed_username_and_aliases`; unit `failed_chat_fetch_is_not_a_confirmed_username_removal`, `successful_chat_without_username_is_authoritative` | 2/2 caught: снятие guard с username → падение на сохранённом username; снятие guard с alias replacement → падение на active alias | `8fb7986` | Полный отказ всех API закрыт в P2.9: `profile_refreshed_at` больше не перезаписывается, retry планируется; production-данные не использовались |
| P2.1 | P2 | #127 `6b24f04` | `src/db/telegram.rs:150`, `:618` | reproduced + fixed | Два первых upsert одного peer с разными username не сериализовались: `SELECT … FOR UPDATE` не блокирует отсутствующую строку, оба видели `previous = None`. После — `pg_advisory_xact_lock` до первого чтения; 32 барьерных итерации и reassignment между двумя owners дают ровно один актуальный alias | `current_username_aliases_resolve_only_fresh_unique_owners` + `url_alias_churn::assert_current_alias_resolution` | — (concurrency test) | `dfd7057` + `426eef1` | Advisory lock сериализует обновления одного Telegram peer; это осознанная цена за согласованность |
| P2.2 | P2 | #127/#131 | `src/features/url_relations.rs:698`, `:944` | fixed | Две модели (`telegram_username_aliases` vs свежие `telegram_user_profiles`) могли расходиться. После — одна authoritative current-ownership таблица/сервис; `Unknown`/`Ambiguous` остаются fail-closed; QR relation не создаёт `Affiliated` при reassignment | `current_username_aliases_resolve_only_fresh_unique_owners`; QR/URL relation tests | — | `426eef1` + `7906e2d` | 24h freshness и SQL privilege `nedobot_mcp_ro` проверены на synthetic PG; production-данные не использовались |
| P2.3 | P2 | #166 `bdc7dd1` | `src/features/chat_read_api/query.rs:1228`; `tests/chat_db_mcp.rs:360` | confirmed coverage gap + fixed | Property генерировал только `1..=i64::MAX`, а Telegram group IDs отрицательны. После генерируются `-i64::MAX..=-1` и явные `-100…`/`-2003`; два инстанса на двух БД не видят чужие messages/notes/profile; mutant scope condition убивается | `generated_forged_chat_ids_are_rejected_by_scope_policy`; `independent_community_databases_generate_manifests_and_start_isolated_mcp_servers` | 3/3 caught | `b5e7a25` | Тест synthetic PG; cursor/filter scope проверяется существующими MCP-тестами |
| P2.4 | P2 | #170 | `src/features/search/mcp.rs:402` | confirmed + fixed | `fetch_urls(..., top_n = 0)` добавлял URL до проверки лимита и возвращал один элемент; после — ранний `Vec::new()`; порядок/дедуп при `top_n > 0` не изменены | `fetch_url_limit_zero_returns_no_targets` | — | `7906e2d` | — |
| P2.5 | P2 | #170 | `src/features/search/mcp.rs:563` | confirmed data exposure path + fixed | В warning писался сырой `url` с path/query/token. После логируются source, host и failure code; URL path/query не пишутся. Тест `https://example.com/private?token=SYNTHETIC_SECRET` доказывает отсутствие секрета | `fetch_failure_event_does_not_include_url_path_or_query_secret` | — | `7906e2d` | Host остаётся в логе намеренно; это публичная часть URL |
| P2.6 | P2 | #173 `f6f552a` | `.github/workflows/quality-nightly.yml:33`; `scripts/run_quality_nightly.sh` | fixed | Nightly запускал 3 seed только для `adversarial_harness`; T07–T14 в `--lib`/`--bin`/`postgres_migrations` не стрессировались. После — 28 явных selectors × 3 fixed seed = 84 executions, manifest `SHA/seed/case`; stateful PG job пишет свой target-level manifest | `sh scripts/run_quality_nightly.sh` (84/84 passed); checked runner отвергает 0 tests | — | `314f26e` | DB/fake-subprocess cases остаются в отдельной disposable lane с bounded timeout |
| P2.7 | P2 | #154/#173 | `scripts/test.sh:18` | fixed | Локальный script не запускал URL-shadow lock/cancellation и URL inventory PG scenarios, которые были только в CI. После — 17 checked invocations, включая shadow lock/cancel, inventory, private history и moderation | `sh scripts/test.sh`: 779 workspace + integration, 0 failed; 17 checked invocations | — | `314f26e` | Локальный script требует Podman/PostgreSQL на 5433 и не является production-командой |
| P2.8 | P2 | ревью #174 / `8fb7986` | `src/db/telegram.rs:712`, `:634`, `:129` | fixed | `upsert_user_profile`: advisory lock → profile row; `update_user_profile_details`: profile row → observation → advisory lock. Конкурентные writers дают `40P01` deadlock. После — оба берут advisory lock до первого write в profile row; trigger-pause regression проверяет реальные locks | `concurrent_profile_refreshes_share_one_lock_order` (две транзакции, trigger `pg_sleep` после row lock, ожидание по `pg_stat_activity` `PgSleep`) | 1/1 caught: удаление advisory lock в начале `update_user_profile_details` → `deadlock detected` | `8fb7986` | Regression использует synthetic trigger-pause 1s; другие writers alias-таблицы уже берут advisory первым, `mark_user_profile_refresh_error` не берёт advisory и не создаёт цикл |
| P2.9 | P2 | ревью #174 / `38efa22` | `src/db/telegram.rs:987` (`user_profile_needs_refresh`), `:953` (`mark_user_profile_refresh_error`), `:875` (backoff SQL); `src/features/user_profiles/service.rs:34` (`load_user_ids`) | fixed | Оба timestamp не-null, `getChat` timeout, photos/personal channel OK: до — `profile_refresh_error` записан, но planner его игнорирует, `getChat` не повторяется; полный отказ дополнительно ставил `profile_refreshed_at=now()`. После — ошибка гейтит retry по `profile_refresh_retry_at`; backoff 1m → 2m → … → cap 6h; `profile_refreshed_at` остаётся временем последнего успешного снимка; успешный refresh очищает error/count/retry | `partial_profile_refresh_retries_after_bounded_cooldown`: оба timestamp seeded, partial failure, username/alias сохранены, `needs_refresh=false` и `load_user_ids` пуст до cooldown, `true`/попадание после rewinding retry_at; второй сбой даёт шаг 2m, counter=100 → cap 6h, total failure сохраняет timestamp | 4/4 caught: удаление error-условия → нет retry после cooldown; удаление retry-gate → retry до cooldown; `mark_…` снова пишет `now()` → timestamp потерян; `load_user_ids` без gate → batch берёт пользователя раньше срока | `9067a35` + `38efa22` | В `profile_refresh_error` по-прежнему попадает только ошибка `getChat`; транзиентный отказ personal channel/photos не планирует retry (pre-existing). Явный `/userstats` refresh обходит backoff намеренно |
| P3.1 | P3 | #158/#164 | `src/features/first_comment/pipeline.rs:744`; `src/features/spam_review.rs:1130` | risk / unverified | Часть fake TCP-серверов ждёт `accept()`/`join()` без явного deadline; при регрессии возможен hang до общего CI timeout. `rmcp_transport` уже использует `within_e2e_timeout` | Не добавлялся отдельный regression: не доказан реальный hang в текущем suite | — | — | Не входит в P1; рекомендуется bounded accept/teardown отдельным test-hygiene PR |

## 3. Результаты проверок на `38efa22`

Все команды выполнены в дереве интеграции; `TMPDIR` указывал внутрь `target/tmp`.

```sh
cargo fmt --all -- --check
git diff --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo test --no-default-features --workspace --all-targets --locked
sh scripts/test_feature_matrix.sh
sh scripts/test.sh
sh scripts/run_quality_nightly.sh
sh scripts/run_targeted_mutations.sh
python3 tests/release_install.py
python3 tests/reviewed_spam_training.py
python3 tests/test_cargo_test_checked.py
python3 scripts/validate_public_url_corpus.py
python3 -m unittest discover -s tests -p 'test_public_url_corpus.py'
```

| Проверка | Результат |
|---|---|
| `cargo fmt --all -- --check`, `git diff --check` | passed |
| `cargo check --workspace --all-targets --locked` | passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | passed |
| Default workspace/all-targets | **781 passed, 0 failed, 25 ignored** |
| No-default workspace/all-targets | **394 passed, 0 failed, 4 ignored** |
| `scripts/test_feature_matrix.sh` | default/no-default check + gate test — passed |
| `scripts/test.sh` | 17 checked invocations, **0 failed**; workspace 781 passed; все три profile regression прошли |
| `scripts/run_quality_nightly.sh` | **84/84** executions (28 selectors × 3 seeds) |
| Targeted mutations | URL 2/2, MCP scope 3/3, classifier 4/4 caught; worker 4 caught + 1 unviable; router branch-order manual mutant killed; profile partial 2/2 caught; profile lock order 1/1 caught; profile retry 4/4 caught |
| Python release/spam/checked/corpus | 2 + 2 + 4 + 5 passed |
| Coverage baseline (T16) | 42.25% lines, 47.95% regions, 45.91% functions; не является gate |

Versions: `rustc 1.100.0 (0ef1bc386 2026-11-09)`, `cargo 1.100.0`, `cargo-mutants 27.1.0`, PostgreSQL `16.14` (pgvector/pgvector:0.8.2-pg16-bookworm, disposable Podman).

## 4. До/после по ключевым инвариантам

- URL: до — `100.64.0.1`, `198.18.0.1`, `192.0.2.1`, `[2001:db8::1]`, `[::ffff:100.64.0.1]` и `http://@example.com` проходили в MCP-аргументы; после — отвергаются, глобальные literal/домены проходят.
- Classifier: до — `http_503` и вложенный typed 429 могли стать `error`; после — `http_503`, `http_429`, `timeout`, `validation_failed`, а spoof-текст остаётся `error`.
- Router: до — тест мог проверять макрос, не production graph; после — тест вызывает `router::build` и убивает перестановку command/message.
- Worker: до — `record_exit` мог вернуть `None`/потерять fatal exit; после — bounded drain + typed fatal, mutation не проходит.
- No-default: до — 20 failed; после — `392 passed, 0 failed, 4 ignored`; на финальном дереве с новыми unit-тестами — `394 passed, 0 failed, 4 ignored`.
- Alias race: до — два первых upsert могли оставить два active alias; после — advisory lock и ровно один active alias на 32 барьерных итерациях.
- MCP scope: до — property не проверял отрицательные Telegram chat IDs; после — проверяет и убивает mutant scope condition.
- Profile partial fetch: до — ошибка `getChat` при успешных photos обнуляла username, деактивировала aliases и ставила свежий `profile_refreshed_at`; после — источник-специфичные guard-ы сохраняют подтверждённые поля, а успешный `getChat` без username остаётся авторитетным удалением.
- Profile writers: до — `upsert_user_profile` брал advisory lock до profile row, а `update_user_profile_details` — наоборот, что давало `40P01`; после — единый порядок advisory lock → profile row.
- Profile retry: до — частичный отказ `getChat` сохранял данные, но planner видел оба timestamp и больше никогда не повторял запрос; полный отказ ставил `profile_refreshed_at=now()`. После — `profile_refresh_error` + `profile_refresh_retry_at` гейтят retry, backoff растёт 1m → 2m → … → 6h, а `profile_refreshed_at` меняется только при успешном `getChat`.

## 5. Безопасность и границы доверия

Не выполнялись: production PostgreSQL, реальные Telegram-чаты/токены, приватные корпуса, внешние LLM/ASR, живые URL/SSRF targets, SSH, merge и deploy. Использовались только synthetic fixtures, локальный fake HTTP/Telegram, disposable Podman PostgreSQL и фиктивные креды. `src/private_corpus.rs` переименован в `src/url_history.rs`; из тестового пакета удалены команды live DB export, а extraction принимает только synthetic JSONL.

Явно **не доказаны**: DNS rebinding и redirect-to-private для внешнего MCP fetch-провайдера; systemd restart policy; полный набор timeout/teardown для всех локальных fake TCP-серверов (P3.1); production-поведение на реальных данных.

## 6. Незакрытые пункты

1. P1.1 DNS/redirect: нужен контроль адреса фактического соединения и каждого redirect hop либо контракт с доверенным fetch-провайдером. Текущий фильтр проверяет только URL-аргумент.
2. P3.1: часть fake TCP-серверов не имеет bounded accept/read/join; это test hygiene, не production-дефект. Рекомендуется отдельный PR.
3. CI на кодовом SHA `38efa22`: push run [44536297777](https://github.com/Mar2ianen/NedoBot/actions/runs/44536297777) — success (13m50s), PR run [44536310348](https://github.com/Mar2ianen/NedoBot/actions/runs/44536310348) — success (13m57s). Все job-шаги, включая PostgreSQL lanes, прошли. Отчётный коммит docs-only; его exact-head run — в checks PR #174.
4. Ветка не merge/deploy; production `main` не изменён.

## 7. Повторная проверка после первого ревью (2026-11-19)

Первый ревью PR #174 выявил два новых дефекта в обновлении Telegram-профилей. Оба воспроизведены на synthetic disposable PostgreSQL и закрыты в `8fb7986`; повторный ревью снял их и выявил отдельный reliability-дефект P2.9, закрытый в `38efa22`. Production-данные не использовались.

### P1.6. Частичный отказ API больше не удаляет подтверждённый профиль

`refresh_profile` выполняет три независимых запроса. Ранее при ошибке `getChat` и успешном `getUserProfilePhotos` вызов `update_user_profile_details` записывал `username = NULL`, деактивировал aliases и ставил `profile_refreshed_at = now()`. Теперь `UserProfileDetails` несёт `chat_fetched`/`photos_fetched` и ошибку `getChat`; SQL обновляет поля только из успешно полученных источников. При частичном отказе сохраняются username, имя, bio, chat-photo, прежний `profile_refreshed_at` и aliases; обновляются только photo-поля и `profile_refresh_error`. При успешном `getChat` без username aliases по-прежнему очищаются, потому что это подтверждённое удаление.

Failing-before: снятие guard-а с `username` даёт `left: None, right: Some("a…")`; снятие guard-а с alias replacement даёт `a failed getChat must not deactivate the last confirmed alias`. Passing-after: оба теста проходят на финальном дереве. Regression: `partial_profile_refresh_preserves_confirmed_username_and_aliases`, unit-тесты `failed_chat_fetch_is_not_a_confirmed_username_removal` и `successful_chat_without_username_is_authoritative`.

### P2.8. Единый порядок блокировок profile writers

`upsert_user_profile` брал advisory lock, затем блокировал profile row. `update_user_profile_details` сначала делал UPSERT profile row, затем внутри alias replacement брал advisory lock. При конкурентном исполнении PostgreSQL возвращал `40P01 deadlock detected`. Теперь `update_user_profile_details` берёт `lock_telegram_peer_alias_snapshot()` до первого write в profile row, как и `upsert_user_profile`.

Failing-before: удаление новой advisory-блокировки воспроизводит `deadlock detected at line 1130`; regression `concurrent_profile_refreshes_share_one_lock_order` завершает одну из транзакций ошибкой. Passing-after: обе транзакции коммитятся, остаётся ровно один active alias. Синхронизация теста идёт по реальному состоянию PostgreSQL: trigger ставит `pg_sleep` после захвата profile row, тест ждёт `wait_event = 'PgSleep'` в `pg_stat_activity`, затем запускает второй writer.

## 8. Повторная проверка P2.9 (2026-11-19)

Ревью подтвердило P1.6/P2.8 и выявило отдельный reliability-дефект: `user_profile_needs_refresh` не учитывал `profile_refresh_error`. Контрпример: `profile_refreshed_at` и `personal_channel_refreshed_at` не-null, `getChat` timeout, `getUserProfilePhotos` и personal channel успешны. После P1.6 username и aliases сохранены, ошибка записана, но planner видел только timestamp-ы и возвращал `false`, поэтому `getChat` больше не повторялся. Дополнительно `mark_user_profile_refresh_error()` при полном отказе ставил `profile_refreshed_at=now()`, хотя успешного снимка не было.

Исправление в `9067a35` + `38efa22`:

- миграция `migrations/20261119120000_user_profile_refresh_retry.sql` добавляет `profile_refresh_failure_count` (default 0) и `profile_refresh_retry_at`; для уже существующих error-rows выставляется count=1 и retry через 1 минуту, чтобы deploy не вызвал burst;
- `user_profile_needs_refresh` и `load_user_ids` считают профиль требующим обновления только когда нет непустого `profile_refresh_error` либо retry уже наступил (`retry_at is null or retry_at <= now()`); legacy-строки с null retry остаются retryable, но новые ошибки всегда получают расписание;
- `update_user_profile_details` при частичном отказе увеличивает `profile_refresh_failure_count` и ставит `now() + least(1m * 2^count, 6h)`, не меняя `profile_refreshed_at`; успешный `getChat` очищает error, count и retry;
- `mark_user_profile_refresh_error` больше не пишет `profile_refreshed_at`, а только error/count/retry, поэтому last successful snapshot сохраняется и после полного отказа.

Failing-before (4 независимые мутации, каждая ломает только один инвариант):

1. удаление `profile_refresh_error is not null` из planner → после cooldown `needs_refresh` остаётся `false` (`the scheduler must retry a partial failure once the cooldown expires`);
2. удаление retry-gate → запрос разрешён до cooldown (`the scheduler must not retry before the cooldown expires`);
3. возврат `profile_refreshed_at=now()` в `mark_user_profile_refresh_error` → `left: Some(2026-…), right: Some(2020-01-01T00:00:00Z)`;
4. удаление retry-gate из `load_user_ids` → batch берёт пользователя раньше срока (`the batch refresher must also respect the cooldown`).

Passing-after: `partial_profile_refresh_retries_after_bounded_cooldown` создаёт профиль с обоими не-null timestamp, воспроизводит частичный отказ `getChat`, проверяет сохранность username/alias, отсутствие retry до cooldown и наличие после rewinding `profile_refresh_retry_at`. Затем проверяет шаг backoff (2 минуты), cap 6 часов при counter=100, сохранение `profile_refreshed_at` при полном отказе и полный сброс error/count/retry после успешного refresh. Тест использует только synthetic IDs и disposable PostgreSQL.

Остаточный риск: в `profile_refresh_error` по-прежнему попадает только ошибка `getChat`; транзиентный отказ personal channel/photos не планирует retry (поведение существует независимо от P2.9). Явный `/userstats` refresh обходит backoff намеренно, потому что это действие пользователя, а не поток сообщений.
