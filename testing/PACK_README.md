# NedoBot adversarial testing pack · v2

**Пакет заданий для Codex, а не готовый fuzz framework.** Здесь 16 самостоятельных задач, один переиспользуемый Codex skill, независимые критерии тестирования, синтетические seeds, шаблоны отчётов, контроль приватности и валидатор структуры пакета.

## Установка пакета

Пакет установлен в NedoBot: `testing/` и `.agents/skills/telegram-input-hunter/`
находятся в корне репозитория. Этот файл сохранён как `testing/PACK_README.md`,
чтобы не заменять основной README проекта.

```bash
python3 testing/tools/validate_pack.py
```

Запусти Codex из корня NedoBot, используя промпт в `testing/START_CODEX.md`. Skill можно вызвать явно: `$telegram-input-hunter`. Модифицировать `AGENTS.md` ради установки не нужно. Для изменения **pure parser** `teloxide-url` Codex должен работать отдельно в `Mar2ianen/teloxide-antispam` и следовать его `AGENTS.md` (в частности, английский для документации/комментариев/коммитов). Скопированные туда приватные материалы запрещены.

## Порядок

- **Фаза A / генерация входов:** T01–T04.
- **Фаза B / семантика Telegram и приватность:** T05–T08.
- **Фаза C / состояния, БД, сбои:** T09–T13.
- **Фаза D / внешние границы и непрерывное тестирование:** T14–T16.

У каждой задачи — собственный файл `testing/tasks/TNN.md`, проверяемые инварианты, исключения и acceptance criteria. Следующая задача не начинается до завершения предыдущей или явного изменения порядка пользователем. Один узкий PR на задачу либо связанный маленький набор — без переписывания архитектуры «по пути».

## Состояние исходников на момент упаковки (2026-11-15)

- `NedoBot#126` был **смержен**: merge SHA `7b8815bbff80c4665b64076a063f5f4919e5465a`.
- `teloxide-antispam#2` был **смержен**: merge SHA `ab720d518e6ecc67149eeab7275339485f6a3aba`.
- Непосредственно перед работой проверить `main` обоих репозиториев; не использовать старые PR head как базу.
- Репозиторий NedoBot: Rust 2024 / SQLx 0.9 / PostgreSQL плюс **локальная SQLite репутация двух инстансов**. Все тесты должны быть офлайн и на изолированных БД.

## Обязательные правила

1. Не обращаться к Telegram Bot API от имени боевого бота, не отправлять сообщения, не тестировать настоящие чаты и внешние сайты.
2. Реальные ссылки NedoNews и ПВО доступны только при **явно разрешённом read-only доступе** и исключительно локально в непрозрачном приватном корпусе. **В архиве реальные данные отсутствуют.**
3. Не публиковать в GitHub/CI артефакты с реальными ссылками, invite hashes, username, id, query, path tokens, raw JSON, медиа, DSN или HMAC ключами. Приватный корпус хранить вне репозиториев.
4. Валидность входа отличать от успешной десериализации (V1/V2/V3): см. `testing/references/VALIDITY_LEVELS.md`.
5. Тест должен проверять семантический инвариант; `does_not_panic` достаточен только для отдельного robustness fuzz lane.
6. Проверять **ненулевое** число тестов, успешно выполненных в реальном CI; особое внимание `#[ignore]` и `--exact`.
7. Не менять prod scoring, бан/капчу, SQLite-хранилище, схему БД или deployment без согласования. Любая находка — минимизированная воспроизводимая регрессия до исправления.

## Файлы

- `testing/START_CODEX.md` — готовое задание для агента.
- `.agents/skills/telegram-input-hunter/SKILL.md` — skill и процесс поиска валидных Telegram входов, ломающих инварианты.
- `testing/tasks/T01.md` ... `T16.md` — 16 последовательных ТЗ.
- `testing/references/` — инварианты, валидность, приватность, state-machine, CI, баг-репорты, методы.
- `testing/corpus/public/` — **только синтетические** seeds с пометками ожидаемой семантики.
- `testing/tools/validate_pack.py` — валидатор структуры файлов, не требует сторонних пакетов.
- `testing/templates/` — неактивные примеры CI; копировать в `.github/workflows` лишь после адаптации.

## Первичные документы

- Telegram Bot API: https://core.telegram.org/bots/api
- Codex skills: https://learn.chatgpt.com/docs/build-skills
- Niri testing: https://github.com/niri-wm/niri/wiki/Developing-niri
- proptest: https://proptest-rs.github.io/proptest/intro.html
- cargo-fuzz: https://rust-fuzz.github.io/book/cargo-fuzz/tutorial.html
- cargo-mutants: https://mutants.rs/
