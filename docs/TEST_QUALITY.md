# TEST_QUALITY.md — качество тестов, а не счётчик успешных assert

## Принцип

**Тест должен ловить правдоподобный дефект.** Большой corpus не компенсирует отсутствие независимого oracle. Тесты по алгоритму-копии проверяют согласованность двух одинаковых ошибок, а не корректность. Тесты не должны проходить из-за пустого выборочного множества или feature gate, который не включился.

## 1. Test design template

Перед кодом каждого значимого PR составить минимальную таблицу:

| Поле | Что записать |
|---|---|
| Invariant | Какое свойство нельзя нарушить |
| Counterexample | Минимальный вход, на котором дефект проявляется |
| Oracle | Откуда знаем правильный результат (spec/protocol/independent fixture) |
| Failure mode | Какое конкретное ошибочное изменение поймает тест |
| Level | unit / integration PG / dptree / concurrency / E2E isolated |
| Reproduce | Seed, case ID, cmd line и версии |

`assert_eq!(f(x), f(x))`, `assert!(true)`, «объект сериализовался» без проверки политики, snapshot без validation исходных инвариантов — не принимаются.

## 2. URL, aliases, QR, hidden entities

- Табличные тесты валидных и невалидных URL, error cases, локали/IDNA, unicode/lookalike, authority/userinfo, wildcard-subdomain, path/query case; независимые ожидаемые canonical + service/resource identities.
- Скрытый `text_link`, видимый `url`, `RichMessage`, QR и несколько источников должны давать ожидаемую **дедупликацию цели**, сохраняя provenance. Один текст с двумя разными hidden targets — не одинаковое evidence.
- Проверять collision-prone и privacy cases: токен/путь/query ни в логах, ни в export, ни в `display_url`; redaction не означает криптографическую нераскрываемость SHA-256 (dictionary attack отдельно учитывать).
- Метаинварианты: `canonicalize(canonicalize(u)) == canonicalize(u)`; добавление несущественного source не увеличивает resource count; permutation invariance на порядок извлечения; редактирование с изменением target меняет fingerprint.
- Ошибочные/случайные домены должны **не** классифицироваться как разрешённые бренды. Регистровый домен ≠ компания ≠ ресурс.
- Строка в corpus без независимого expected outcome — вход для fuzz, **не** regression oracle.
- Интеграция QR: файл сменён до записи, job lease stale, file_id changed, cancel/retry/timeout, no double scan when downstream refresh fails.

## 3. Детерминированная генерация

- Использовать `proptest`/проверенную библиотеку: фиксированный seed/replay case для CI; mutable extra random seed локально необязателен.
- При падении сохранять **минимизированный безопасный case** и команду воспроизведения. Shrink должен реально уменьшать контрпример; проверить один искусственный defect.
- Синтетические fixture URL/Telegram updates: без реальных токенов, email, private chat IDs, конфигов, invite hashes. Redaction и permissions для временных файлов.
- Ограничивать размер/время/память fuzz; corpus и grammar версионировать. Не делать optional ignored тест единственной проверкой опасного кода.
- Не верить 10 000 random кейсам, если они не попадают в branches; измерить покрытие краевых классов и mutant kill для критичных policy.

## 4. Dispatch/authorization

- `dptree::Handler::dispatch` над синтетическими typed deps и fake side effects: отсутствие unmatched branch; `Command` и gateway Ask precedence; обработка unknown/private/secondary chat; `ingest=false` и special Ask; command ingestion exactly once.
- Фильтры проверять через policy outcomes и **число исполненных эффектов**; одна отправка, ноль побочных эффектов после deny; panic/early errors не приводят к Telegram send.
- Ожидаемая очередность явных lifecycle шагов: ingest → observation best-effort → intent → enqueue/dispatch; не полагаться на воображаемый multicast dptree.

## 5. SQL и workflows

- С настоящим disposable PG: concurrent `claim #1`, lease expiry/reclaim, claim #2, stale #1 finalize => `LeaseLost`; barrier/transaction boundaries детерминированные, без sleep-based flaky тестов.
- Повтор update/edit/delete: не увеличивать counts дважды, не оставлять старый fingerprint, не ослаблять operator `not_spam` или chat scope.
- Два конкурирующих админа, ручная санкция и капча, undo на частично исполненном batch, чужой moderator/instance → fail closed.
- При удачной внешней отправке и SQL rollback -> `delivery_unknown`, не повтор; при подтверждённом rejection retry по policy; после cache hit результат не дублирует delivery.
- Сохранять источники истины: review/assessment/score materialization отдельно, success generation не запускает повторный LLM при сбое materialization.

## 6. Unit/integration/CI матрица

- Быстрая ветка: `cargo fmt --all -- --check`; `cargo check --workspace --all-targets --locked`; `cargo test --workspace --all-targets --locked`; `cargo clippy --workspace --all-targets --locked -- -D warnings`.
- Без default features: `cargo check --no-default-features --all-targets --locked`, плюс целевые feature combinations, если они реально поддерживаются.
- SQL code/migrations: `./scripts/test.sh` (внутри disposable PG), отдельные ignored PG tests c `TEST_DATABASE_URL` по `.github/workflows/ci.yml`; убедиться, что filtered tests реально существуют/исполнены.
- Дополнительные инструменты: `cargo-nextest` — ускорение unit/integration, не замена doctests; `cargo-mutants` — выборочный mutation testing критичного policy, не жёсткий CI gate до baseline; `cargo-fuzz`/`libfuzzer` — отдельная периодическая sandbox lane по бюджету.
- Из-за возможных изменений инфраструктуры CI не менять всю matrix/compile-feature поверхность одним PR. CI не должен запускать внешние API.

## 7. Definition of done для тестов

- Тест ловит хотя бы один описанный regression или mutation.
- Нет skipped/filtered zero cases под видом успеха.
- Failure report воспроизводим локально: seed, минимальный кейс, expected/actual, без секретов.
- DB tests изолированы от production и друг от друга.
- Для изменённого use case есть normal, adversarial, failure, timeout/retry, replay/duplicate, concurrency/scope paths по применимости.
- Результат CI + локальные ограничения перечислены явно, не «всё ок».

## 8. Качественный review checklist

Спроси: «Если разработчик случайно поменяет `deny` на `allow`, опустит revision check, перенесёт send после commit, проигнорирует одно hidden entity или разрешит `youtube.com.evil.tld`, **какой именно тест покраснеет?**». Если ответа нет — тестов недостаточно, даже если coverage 100%.
