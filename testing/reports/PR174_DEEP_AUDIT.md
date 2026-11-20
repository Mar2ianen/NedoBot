# PR #174: углублённый аудит — рабочий отчёт

Исходный SHA: `c3571fed1e30dd2c34d07d48f0a403d164b11aea`. Рабочая ветка:
`codex/integration-audit-126-173`. P1.6/P2.8/P2.9 повторно не открывались.
Ниже отдельно указаны внесённые изменения и фактически полученные доказательства;
результаты на рабочем дереве не считаются CI на точном SHA.

| Находка / кейс | Путь и контрпример | Контракт и доказательства | Статус |
|---|---|---|---|
| P1-A / ID-01, ID-02 | `manual_moderation::handle` → поиск кандидата → `checked_targets`; имя бывшего владельца осталось локально, а `getChatMember` возвращает другое имя. Старый batch хранил только ID. | Сохраняются источник выбора, имя и ID; свежий уникальный кандидат проверяется через `getChatMember`. Незавершённые старые batch с ID без provenance не возобновляются. Регрессия `username_sanctions_require_live_ownership_and_resume_preserves_provenance` использует PG и loopback Bot API. | Локальная регрессия прошла: ноль эффектов для ban/mute/warn и resume с несовпадением; reply и ID дошли до fake API для ожидаемого пользователя. Mutation proof не выполнен. Сверка и санкция — отдельные Telegram API вызовы. |
| P1-B / QUEUE-01 | `save_telegram_message` → `try_enqueue` → `Full`/`Closed`; раньше commit сообщения не сохранял заявку. | Durable intent пишется вместе с сообщением и активностью участника, повтор того же сообщения не увеличивает ревизию. Reconciler ограничен instance и разрешёнными чатами. Регрессия `saturated_and_closed_queue_preserves_ingested_audit_intents`: 259 synthetic сообщений, включая `Full`, `Closed` и commit без отправки в очередь; все 259 intents и baseline появились, повтор ingest не создал вторую ревизию. | Локальная регрессия прошла. Остаточные риски: replay материала между commit и подтверждением intent; reconciler использует сохранённый профиль, когда API refresh не состоялся; retry исчерпывается в явный terminal marker. Mutation proof не выполнен. |
| P2-C / QR-01 | `qr_url::process_job` вызывал pure classifier; известный внешний Telegram username URL становился `unresolved`. | Worker после loopback download и реального decode вызывает общий свежий resolver; `qr_worker_persists_fresh_alias_relation_and_rejects_superseded_media` проверяет persisted relation, external count и устаревшее медиа. | Локальная регрессия прошла: external, affiliated, ambiguous/stale unresolved, superseded не записан. Mutation proof не выполнен. |
| P2-D / LEASE-01 | `mark_post_comment_pre_send_failed` принимал истёкший processing lease до reclaim. | Предикат живого lease применяется только к `processing`, не меняя политику подтверждённого reject после `sending`. Регрессия включена в `clean_test_database_applies_migrations_and_preserves_comment_job_lifecycle`. | Локальный PG-контур: 7 passed, включая этот тест. Mutation proof не выполнен. |
| P2-E / QUEUE-02 | `process_profile_refresh_job`: два чата могли начать два fetch одного профиля до DB-lock. | Worker берёт локальный lock до проверки freshness и удерживает до завершения записи; отдельно выполняется audit каждого чата. `two_chats_share_profile_fetch_but_keep_both_audits` удерживает первый fake API запрос и запускает второй job до release. | Локальная регрессия прошла: один вызов, актуальный alias, baseline обоих чатов. Отдельные процессы и другие точки вызова refresh этим lock не защищены. |
| P2-F / STATS-01 | `/userstats` выбирал последний `updated_at` даже при повторном использовании имени. | Общий свежий resolver, затем проверка локального `telegram_chat_users`; явные ID/reply без изменений. `assert_user_stats_username_uses_fresh_unique_alias` проверяет реальный `user_stats_report_data`. | Локальный PG-контур: 7 passed, включая кейсы reuse, ambiguity, stale, cross-chat, ID и reply. Mutation proof не выполнен. |
| QUALITY-01 | Другой тест мог дать ненулевой общий pass; фиксированные проверки считались seeded. | Replay проверяет точный список имён и Cargo target, пишет их в TSV; тесты с неизменяемыми входами выполняются один раз. `python3 tests/test_cargo_test_checked.py`: 5 passed, включая отказ при другом имени/target. | Локальный `scripts/run_quality_nightly.sh` прошёл; exact-head CI не подтверждён. |
| DIFF-01 | Reference-model trace не проверял SQL adapter. | `post_comment_sql_matches_reference_lease_and_delivery_trace` прогоняет общие операции над `DurableJobModel` и реальным SQL: сравнивает статус, attempts, разрешение на отправку и итог неопределённой доставки. Реальной отправки нет. | В локальном `scripts/test.sh` дифференциальный тест прошёл. |
| COVER-01 | Старый агрегат не измерял exact head и не включал ignored PG lanes. | В opt-in nightly добавлены изолированная PG lane, separate LCOV, TSV покрытия добавленных строк и SHA артефакта. Непроверенные critical branches перечислены ниже. | Конфигурация внесена; артефакт не собран, branch coverage не измерено. |

## Воспроизведение

Сквозная проверка P1-A обнаружила, что исходный `authorize_actor` отклонял
любую команду, требующую права `can_restrict_members`, ещё до `getMe`: он передавал
`bot_can_restrict=false` в общий предикат до проверки бота. Поэтому исходный
контрпример не достигал санкции через `handle` на аудируемой голове. Исправление
восстанавливает задуманную двухшаговую проверку (сначала актор, затем реальные
права бота) вместе с проверкой имени, а не вместо неё.

`TMPDIR=$PWD/target/tmp sh scripts/test.sh` пересоздаёт только disposable локальные
тестовые БД и запускает новые ignored PG cases. Для точечного прогона после миграций
передать локальный `TEST_DATABASE_URL` и использовать `cargo test --locked --lib
<полное_имя> -- --ignored --exact`. Fixtures синтетические (Telegram V2);
QR PNG воспроизводится командой `qrencode -s 5 -o tests/fixtures/deep_audit_qr.png
'https://t.me/fixture_qr_alias/123'`; ссылка не открывается. Ни production DB,
ни Telegram, ни внешние MCP/LLM, ни приватные корпуса для этих тестов не использовались.

Локальные проверки: `cargo clippy --workspace
--all-targets --all-features --locked -- -D warnings` и `cargo check --workspace
--all-targets --no-default-features --locked` прошли. Полный `scripts/test.sh`
завершился с кодом 0: workspace-этап 781 passed, 0 failed, 30 ignored;
после него 31 успешное исполнение в PG-ланах (включая повторные селекторы).
Отдельный default-disabled прогон workspace/all-targets: 394 passed, 0 failed,
4 ignored; `scripts/test_feature_matrix.sh` прошёл для default и no-default.

Nightly: 58 запусков селекторов, 62 записи успешных тестов в
`target/quality/executed-tests.tsv`; 9 фиксированных селекторов выполняются один
раз, а не трижды. SHA в локальном TSV указывает базовый commit, поверх которого
были незакоммиченные изменения: это не exact-head artifact.

### Критические ветки, где ещё нет доказательства покрытия

- ID: изменение имени между последней сверкой и Bot API effect, старый batch
  без provenance в полном Telegram adapter.
- QUEUE: остановка после commit audit, но до ACK intent; состязание повторных
  claims после expiry; несколько процессов, обновляющих один глобальный профиль.
- QR: недоступность alias storage во время materialization; отдельная проверка
  retry после такого сбоя.
- STATS: явные ID/reply намеренно сохраняют прежний scope; новый scope применяется
  только к имени.
