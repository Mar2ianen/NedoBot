# План T01..T16

| Шаг | Репо | Проверяемая область | Зависимость |
|---|---|---|---|
| T01 | NedoBot + antispam | каркас, replay, seeds, отчёты | — |
| T02 | NedoBot | приватный исторический корпус и 2 инстанса | T01 |
| T03 | antispam | URL/URI grammar, canonicalization | T02 |
| T04 | antispam + NedoBot | Telegram link taxonomy, peer resolver, alias churn | T03 |
| T05 | antispam + NedoBot | UTF-16 entities, rich text, keyboards | T04 |
| T06 | NedoBot | Bot API-valid Update и обработчики | T05 |
| T07 | NedoBot | QR/media генерация, ресурсы | T06 |
| T08 | NedoBot | экспорт/приватность/метки/корпус | T07 |
| T09 | NedoBot | reference model + random event sequences | T08 |
| T10 | NedoBot | QR jobs, edit, lease и stale-write | T09 |
| T11 | NedoBot | PG lock/cancel/interleavings | T10 |
| T12 | NedoBot | SQLite WAL + две инстанции | T11 |
| T13 | NedoBot | изоляция двух PG, permissions, MCP | T12 |
| T14 | NedoBot | threat feed, SSRF, redaction in errors | T13 |
| T15 | оба | cargo-fuzz, seed/cmin/tmin, sanitizer | T14 |
| T16 | оба | long-run proptest, coverage, mutation, nightly CI | T15 |

**Каждый шаг:** независимые expected outcomes; V1/V2/V3; отсутствие интернета и боевого доступа; статистика выполнения; чёткие acceptance gates; воспроизводимые регрессии. Учёт тестов с `#[ignore]` обязателен.

Код NedoBot и его зависимостей — изменяемая цель; все конкретные пути и названия функций перед исполнением сверить с текущим main. Если задача требует новая схема/миграция — отдельно согласовать и сначала доказать, что существующая схема недостаточна.
