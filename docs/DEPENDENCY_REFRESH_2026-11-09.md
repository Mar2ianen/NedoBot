# Обновление зависимостей 9 ноября 2026

Обновление готовится поверх instance-scoped MCP, AgentProfile, conversational
memory и opt-in mention/reply gateway. Все community-инстансы используют один
release binary и отдельные конфигурации/БД.

| Зависимость | Было | Стало |
| --- | --- | --- |
| teloxide fork | 0.20.0, `1c33feff` | 0.21.0-rc.1, `91ec68b1` |
| teloxide-core | 0.16.0 | 0.17.0-rc.1 |
| genai | 0.6.5 | 0.7.3 |
| RMCP | 3.5.1 | 3.9.0 |
| SQLx | 0.8.6 | 0.9.0 |
| jsonwebtoken | 9.3.1 | 11.1.0, явный `rust_crypto` |
| SHA-2 | 0.10 | 0.11.1 |
| TOML | 0.8.23 | 1.2.1 |
| Tokio | 1.52.3 | 1.54.1 |

Остальные совместимые registry-зависимости обновлены в `Cargo.lock`.
`teloxide-antispam` и `teloxide-statistics` сохраняют согласованные integration
пины: default branch statistics ещё не содержит используемый shared activity
model, а default branch antispam ссылается на более старый statistics. Простая
замена этих двух refs откатывает возможности и создаёт несовместимые копии типов.

## Изменения приложения

- Адресаты всех обычных, rich и файловых отправок перенесены в `.chat_id(...)`
  согласно [migration guide fork-а](https://github.com/Mar2ianen/teloxide-fork/blob/91ec68b19efd5e490fb27341a039ce0175b16929/MIGRATION_GUIDE.md).
  Параметры ephemeral-получателя и forum topic сохранены.
- Локальная ошибка сериализации запроса классифицируется как подтверждённый
  отказ до доставки. Неизвестный результат сетевой отправки по-прежнему не
  разрешает повторную отправку.
- Rich TON address сохраняет видимый текст при ingest.
- Старый application workaround для Gemini Schema удалён: genai 0.7.3
  отправляет полный JSON Schema через `parametersJsonSchema`. Wire-тест
  проверяет сохранение nullable types и параметра с именем `additionalProperties`.
- Ошибки инициализации genai client передаются вызывающему коду без раскрытия
  endpoint/секретов; fallible builder не превращён в `unwrap` в runtime.
- Динамические SQLx-запросы явно отмечены `AssertSqlSafe` после проверки:
  SQL-фрагменты происходят из static policy или reviewed MCP catalog,
  пользовательские значения остаются позиционными параметрами. Quoting имён
  views в генераторе manifest экранирует двойные кавычки.
- Release artifact включает `migrate`, `provision_mcp_instance_scope`,
  `generate_mcp_db_manifest`, `validate_instance_config`. Installer проверяет
  наличие всего набора до замены файлов в обоих инстансах. Перед restart workflow
  проверяет оба operator-owned профиля через штатный parser EnvironmentFile.

## Первая выкладка после PR #100–#102

До рестарта создать резервные копии обоих PostgreSQL databases, manifest,
профилей и старых бинарников. Использовать только CI artifact, собранный на
Ubuntu 24.04; локальные бинарники с более новым glibc в production не переносить.

1. Выполнить `validate_instance_config` с EnvironmentFile и `LLM_PROFILES_PATH`
   каждого сервиса. Профили и секреты не заменять repository example-файлом.
2. Применить миграции из того же artifact бинарником `migrate` отдельно в обеих
   БД. Исторические миграции не менять.
3. Проверить scope через `provision_mcp_instance_scope` с профилем каждой БД.
   Для первой явной перепривязки PVO требуется `--confirm-scope-change`.
   Перед перепривязкой остановить MCP-процессы этой БД. На последующих релизах
   несовпадение scope — ошибка, а не причина автоматически менять его.
4. Сгенерировать отдельные manifest, проверить scope и reviewed типы/колонки,
   установить в operator-owned пути и задать соответствующий `MCP_MANIFEST`
   каждому инстансу. PVO не должен использовать NedoNews manifest.
5. Для уже существующего публичного NedoNews endpoint явно подтвердить
   `[public_mcp] enabled = true` только в его профиле. В остальных профилях
   оставить выключенным. Новые gateway-триггеры не включать автоматически.
6. Выполнить release deploy на merged main; проверить все три unit, хеши
   работающих executable, отсутствие новых NRestarts/ошибок, последнюю миграцию
   в обеих БД и MCP discovery с каждой отдельной read-only конфигурацией.
7. Зафиксировать точный SHA, immutable deployment tag, artifact checksum,
   workflow и окно наблюдения в `docs/DEPLOYMENT.md`.

Миграции additive, однако откат бинарника не отменяет изменения MCP views.
Для rollback восстановить совместимые manifest/профили из backup и проверить
MCP scope заново; историю `_sqlx_migrations` не переписывать.
