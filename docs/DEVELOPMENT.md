# Локальная development PostgreSQL

Для миграций и PostgreSQL integration tests используется отдельный локальный контейнер. Он не использует production container, volume или порт и не содержит production-данных.

## Управление базой

Нужен установленный `podman`.

```bash
./scripts/dev_db.sh start
./scripts/dev_db.sh status
./scripts/dev_db.sh stop
```

Контейнер называется `tg-ai-bot-postgres-dev`, данные хранятся в named volume `tg-ai-bot-postgres-dev-data`, PostgreSQL слушает только `127.0.0.1:5433`.

Development DSN:

```text
postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_dev
```

Учётные данные намеренно только для локальной development базы. Не использовать их в production и не заменять ими production secrets.

## Миграции development базы

После запуска контейнера применить все SQLx migrations:

```bash
DATABASE_URL=postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_dev cargo run --bin migrate
```

Команда `migrate` подключается только по `DATABASE_URL` и не запускает Telegram polling, LLM или внешние API.

## Полный test suite с PostgreSQL

```bash
./scripts/test.sh
```

Runner сам выполняет следующие действия:

1. поднимает локальный контейнер, если он остановлен;
2. пересоздаёт отдельную test database `tg_ai_bot_test`;
3. применяет migrations к этой чистой базе;
4. запускает `cargo test --all-targets`;
5. запускает ignored PostgreSQL integration tests с `TEST_DATABASE_URL`.

Test database не совпадает с development database, поэтому integration tests могут свободно вставлять и удалять данные. Обычный `cargo test` не стартует Podman и не требует PostgreSQL.

## Сброс

Сбросить только test database:

```bash
./scripts/dev_db.sh reset-test
```

Полностью удалить локальный контейнер и его данные:

```bash
./scripts/dev_db.sh reset
```

`reset` удаляет только локальный контейнер и volume `tg-ai-bot-postgres-dev-data`. Перед командой убедитесь, что `podman ps` не показывает production container под этим именем.

## Политика данных

Пустая база с миграциями — штатный target для integration tests. Если для конкретного расследования понадобится production snapshot, создавать его следует только вручную, после удаления личных данных и без коммита export/dump в репозиторий.
