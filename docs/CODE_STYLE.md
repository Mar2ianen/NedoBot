# CODE_STYLE.md — Rust инженерный кодстайл NedoBot

## Цель

Рефактор направлен на понятные границы, явные переходы состояния, исчерпывающую типизацию и проверяемое поведение. Это **не** соревнование по сокращению LOC и не повод вводить новые глобальные абстракции. `rustfmt` — обязательный базовый слой; ручной стиль не конфликтует с форматтером.

## 1. Модули и зависимости

- `main/app` — композиция, миграции и lifecycle; `telegram/router` — транспортные маршруты; `features/<domain>` — конкретный use case/policy/repo; `db` — I/O boundary; `llm/mcp` — инфраструктурный adapter.
- Направление: adapters → application use cases → domain; application → ports; infrastructure implements ports. Не строить циклических зависимостей между features.
- Новая фича живёт в собственном модуле и подключается через один declarative route и, если нужно, job registration. Новые ветки в `handle_message`/`handle_command` — только как миграционный мост, который должен исчезнуть.
- Запрещена абстракция «универсальный service manager» без фактических потребителей. Непонятный generic хуже двух небольших очевидных функций.

## 2. Типы и enum

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryCertainty { NotAttempted, Rejected, Unknown, Confirmed }

enum EffectPlan { NoOp, Enqueue(AuditJobRequest), Respond(ReplyContent) }
```

- В закрытом business state использовать `enum` вместо `String`/`&str` и каскадов boolean flags.
- `match` по enum — по возможности перечисляет все варианты. Wildcard запрещён в safety-critical policy, где новая ветка должна ломать компиляцию.
- Хранение в SQL `text` допустимо, но только конвертер/`sqlx::Type` на boundary. Unknown persisted status -> `Result::Err` (или явно видимое Unknown read-model), не `unwrap_or(Pending)`.
- Использовать `newtype` для `ChatId`, `MessageRevision`, `SourceId`, `JobFence` в новых доменных API там, где предотвращается путаница. Не делать огромный перевод всех идентификаторов в один PR.
- Не использовать bool flags `send: bool, check_scope: bool, auto: bool` в сигнатурах, когда лучше `enum Mode`/`enum Policy`.
- `Option` = реальная необязательность, не многозначное состояние; impossible states не должны компилироваться.

## 3. Ошибки

- При recoverable/failure policy применять typed `enum DomainError` c `thiserror` (если уже в dependency graph; иначе оценить добавление).
- `anyhow` на top-level adapters/CLI допустим; не извлекать тип ошибки через `.to_string().contains(...)`.
- В диагностике наружу — bounded safe error kind, без raw provider body, Telegram token, URL query/invite и PII.
- Явно разделять `ConfirmedRejected` и `DeliveryUnknown`; условный `timeout` не всегда означает, что Telegram запрос не принят.
- Не использовать `.unwrap()` в production path кроме доказанных compile-time/nonuser invariants с объяснением.

## 4. Async и Tokio

- Никаких неограниченных `spawn`/unbounded mailbox на каждый message/update. Bounded queues и backpressure с политикой Full/Closed/Coalesced.
- Не держать mutex/DB transaction во время LLM/network I/O; исключение явно обосновать fencing/exclusive processing.
- Все долгоживущие workers регистрируются в supervisor и имеют shutdown policy; lease reconciliation остаётся на domain boundary.
- Не путать отмену async future и остановку `spawn_blocking`/subprocess: проверять cancellation/cooperative abort и утилизацию ресурсов.

## 5. Данные/SQL

- SQL bind placeholders, строго allowlisted dynamic identifiers, scopes по `(instance, chat, user, message)` где применимо.
- Новые migrations только additive, upgrade path тестировать. Не изменять прежние migrations.
- Сохранять `FOR UPDATE SKIP LOCKED`, monotonic attempt/fence, snapshot revision и CAS ordering.
- Предварительный план external effect должен иметь idempotency policy. Считать `at-most-once` vs `at-least-once` vs `unknown` явно.
- Не создавать универсальный SQL jobs repository вместо отдельных domain schemas.

## 6. Тестируемость

- Чистые функции не принимают `Bot`/`PgPool`; внешний I/O отделять портом лишь когда есть настоящий seam в тестах.
- Извлекать `Clock` или фиксировать время лишь для проверки edge cases; не навязывать clock trait всему проекту.
- Prefer typed fixtures/builders с defaults; не копировать `Config` на сотни полей в каждый test.
- Unit tests именуются по инварианту/регрессии, а не реализации. См. `TEST_QUALITY.md`.

## 7. Реюз зависимостей — правило до написания кода

- Сначала `rg` по проекту и `Cargo.toml/Cargo.lock`, затем docs.rs/crates.io/upstream; уже существующая реализация имеет преимущество при равной пригодности.
- Запрещены собственные реализации URL parsing/normalization/IDNA/PSL, Telegram entities/wire protocol, JSON-RPC/MCP, QR decoder, HTTP client generic retry, `yt-dlp`, media transcoder, SQL migration framework, cron/calendar, async execution runtime при наличии зрелой библиотеки.
- Исключения: действительно крошечный чистый алгоритм с очевидным unit-test; отдельная обоснованная security-boundary реализация; proof-of-concept, оставшийся вне production.
- Для дополнительного crate документировать: задание; почему существующие crates не подходят; API compatibility; maintenance; license/MSRV; unsafe/transitive deps; adversarial behavior; evidence от тестов. Не превращать dependency count в религию: если библиотека избыточна, маленький helper лучше.

## 8. Организация функций

- Не считать 60 строк магическим hard limit. Длинная функция оправдана, если она композиция с линейным flow и очевидными эффектами. Выносить **семантические этапы** с отдельными контрактами.
- Предпочитать early return, `?`, pattern matching, `is_some_and`, named DTO; избегать `map/unwrap_or` где маскируется ошибка.
- Не делать абстрактный enum только из одного варианта «на будущее» и не создавать extra crates без DAG-основания.
- Trace spans: message/job/chat/attempt/revision с safe identifiers, без payload и секретов.

## 9. Документация и PR

- Публичный контракт — Rustdoc/ADR в месте принятия решения, а не копирование всей архитектуры в каждый файл.
- Коммиты на английском imperative с типом `refactor:`, `test:`, `docs:`, `fix:`. Отдельный functional bugfix не смешивается с refactor.
- Новая фича/команда: обновить `BotCommands`, отдельный router endpoint, feature gate, tests, docs; **не** добавлять реализацию в общий `command_handler.rs`.

## 10. Cargo features и совместимость

- Runtime-конфигурация выбирает включённые возможности для конкретного community instance; compile-time Cargo features — отдельная граница доступности модулей, а не переключатель профиля или способ закодировать поведение одного из чатов.
- Структурный рефакторинг не добавляет пользовательских возможностей и не меняет существующие feature gates. Shorts/TikTok в архитектурных примерах не является разрешением реализовывать загрузчик или продуктовый маршрут.
- Минимальная матрица для изменения Rust-кода: default workspace `fmt`, `check --workspace --all-targets --locked`, `test --workspace --all-targets --locked`, Clippy `-D warnings`; проверять `--no-default-features` для каждого затронутого optional module/build path. Изменения feature gates или config validation требуют теста как default, так и отключённого состояния.
- PostgreSQL семантика проверяется с миграциями на disposable БД. Не подключать тесты к production DSN и не считать mock заменой для SQL scope/lease/constraint поведения.
