# Базовая картина качества NedoBot

Дата измерения: 2026-11-17

Исходный `main`: `7b8815bbff80c4665b64076a063f5f4919e5465a` (2026-11-15)
Задача: T00 из пакета `NedoBot_full_testing_T00-T17_OpenCode_2026-11-16.zip`.

## Среда и границы измерения

- Rust `1.99.1 (58cf8f9c3 2026-10-13)`, Cargo `1.99.1`, LLVM в rustc `23.1.3`.
- Системные `/usr/bin/llvm-cov` и `/usr/bin/llvm-profdata`: `23.1.4`; `cargo-llvm-cov 0.9.1` установлен вне репозитория.
- PostgreSQL-интеграции запускались только против локального Podman PostgreSQL на `127.0.0.1:5433`, с disposable базами `tg_ai_bot_test` и `tg_ai_bot_test_b`. `scripts/dev_db.sh reset-test` не сбрасывает `tg_ai_bot_dev`.
- Не использовались production DB, Telegram, пользовательские URL, внешние LLM/ASR/MCP endpoint или приватные данные.
- На актуальном `main` отсутствуют `docs/CODE_STYLE.md` и `docs/TEST_QUALITY.md`, на которые ссылается архив. Прочитан корневой `AGENTS.md`; локальные правила для стиля карточек и SQL/HTML/секретов также остаются обязательными.

## Фактически выполненные проверки

| Команда | Результат |
|---|---|
| `cargo test --workspace --all-targets --locked -- --list` | Обнаружено 672 теста: 659 обычных и 13 `#[ignore]`; это inventory, тестовые тела не запускались. |
| `CARGO_BUILD_JOBS=4 cargo test --workspace --all-targets --locked` | Успех: 659 passed, 0 failed, 13 ignored. Инкрементный test-profile: 0.31 s; сумма `finished in` по test binaries: 4.13 s. |
| `CARGO_BUILD_JOBS=4 cargo test --no-default-features --workspace --all-targets --locked` | Неуспех: root library — 230 passed, 20 failed, 1 ignored. Test-profile: 42.73 s; тесты завершились за 0.18 s. Сначала падают проверки config на включённые в production-профиле фичи, скомпилированные без этих фич; затем часть тестов ломается из-за poisoned env mutex. Это baseline-дефект тестовой конфигурации, не исправлявшийся в T00. |
| `CARGO_BUILD_JOBS=4 ./scripts/test.sh` | Успех. Пересозданы только две disposable test DB; миграции на обеих применены. Начальная компиляция test-profile: 1m17s; migration lifecycle — 33.17 s. Прошли workspace all-targets, migration lifecycle, URL evidence, два URL-shadow сценария, три manual moderation и два MCP-теста. |
| CI-only PostgreSQL lanes, отсутствующие в `scripts/test.sh` | Дополнительно вручную успешно выполнены URL-shadow serialization (0.15 s), cancellation lock-release (0.10 s) и URL inventory privacy/labels (0.19 s) на тех же disposable DB. |
| `LLVM_COV=/usr/bin/llvm-cov LLVM_PROFDATA=/usr/bin/llvm-profdata CARGO_LLVM_COV_SETUP=0 CARGO_BUILD_JOBS=3 cargo llvm-cov --workspace --all-targets --locked --summary-only` | Успех; compile+run: 3m34s. Сводно: 45.24% regions, 46.76% functions, 42.70% lines. Branch coverage недоступно (вывод `-`). |

При `--no-default-features` 20 падающих тестов:

- Конфигурация: `enabled_ask_requires_owner_model_and_mcp_command`, `ask_validates_invites_for_every_ask_chat`, `committed_production_profile_has_valid_community_config`, `risk_captcha_is_disabled_by_default_and_validates_its_threshold`, `from_env_defaults_ask_db_mcp_env_to_database_url_and_manifest`, `enabled_voice_pipeline_requires_asr_key`, `profile_mode_keeps_voice_asr_secret_validation`, `profile_mode_uses_route_profiles_for_memory`, `profile_mode_validates_route_fallback_secret_environment_names`, `new_user_audit_profile_validation_requires_prompt_only_fallback_secret`, `mcp_scope_and_public_gate_come_from_selected_community_profile`, `runtime_settings_defaults_to_disabled_audit_and_reject_invalid_values`.
- Поиск и MCP: `fetch_urls_skips_blocked_source_domains`, `extract_error_returns_skipped_extract_failed`, `disabled_returns_skipped_disabled`, `enriches_youtube_result_with_yt_dlp_vtt`, `blocks_exact_domain_and_subdomains_without_overmatching`, `github_tool_names_keep_only_readonly_search_tools`, `default_policy_contains_multiple_blocked_sources`, `blocks_configured_terms_in_comment_text`.

Первопричина большинства ошибок — production TOML активирует community features, отсутствующие в `--no-default-features` бинарнике; тесты падают при загрузке общей конфигурации раньше проверяемого условия. Первая паника удерживает общий env mutex в poisoned-состоянии, что добавляет каскадные ошибки. Проверки фич в T15 должны использовать независимую тестовую конфигурацию и изолированную среду переменных.

## Игнорируемые тесты и их выполнение

В обычном workspace run игнорируются 13 тестов:

| Группа | Число | Запуск в этом baseline |
|---|---:|---|
| URL evidence PostgreSQL (`src/db/url_evidence.rs`) | 1 | Да: точная команда в `scripts/test.sh` |
| New-user URL shadow PostgreSQL (`src/features/new_user_analysis.rs`) | 4 | Да: privacy, QR/removal, lock serialization и cancellation. Две последние — дополнительными командами как в CI, потому что `scripts/test.sh` их не содержит. |
| Manual moderation PostgreSQL (`tests/manual_moderation.rs`) | 3 | Да: `--features manual-moderation --test manual_moderation -- --ignored` |
| MCP PostgreSQL (`tests/chat_db_mcp.rs`) | 2 | Да: две отдельные точные команды `scripts/test.sh`; обе БД локальные disposable. |
| Миграции PostgreSQL (`tests/postgres_migrations.rs`) | 1 | Да: `--test postgres_migrations -- --ignored` |
| URL inventory PostgreSQL (`src/bin/export_url_inventory.rs`) | 1 | Да: дополнительная CI-команда, отсутствующая в `scripts/test.sh`. |
| Live `/ask` smoke (`src/features/ask/agent.rs`) | 1 | Нет: требует production-like DB, MCP и LLM конфигурации; не подходит для безопасного локального baseline. |

Итого: все 12 безопасных локальных ignored-тестов запущены и прошли; один live smoke намеренно пропущен. Команда для полного воспроизведения PostgreSQL-набора: сначала `CARGO_BUILD_JOBS=4 ./scripts/test.sh`, затем три команды, приведённые в `testing/critical-branch-matrix.md` для URL-shadow locks и inventory. Базы disposable; не подставлять production DSN.

## CI и feature matrix на исходном HEAD

`.github/workflows/ci.yml` выполняет default `cargo test --workspace --all-targets --locked`, Clippy и PostgreSQL lanes для URL evidence, URL shadow, export inventory, migration lifecycle, MCP и manual moderation. Отдельной `--no-default-features` lane и coverage lane нет. Локальный `scripts/test.sh` запускает основную часть набора, но пропускает два URL-shadow lock/cancellation сценария и URL inventory; последние сейчас есть только в CI.

Coverage summary включает код бинарников без `main`-level тестов, поэтому, например, CLI `migrate` имеет 0% строк, а результат workspace снижает неисполненный glue-код. Среди чувствительных участков текущая line coverage низкая: `src/telegram/command_handler.rs` 22.99%, `src/mcp/tools/db.rs` 5.85%, `src/features/new_user_audit/repo.rs` 5.09%, `src/telegram/manual_moderation.rs` 10.91%. Интеграционные тесты частично закрывают эти ветви отдельными DB lanes, но сводный `cargo llvm-cov` не подключает их переменные окружения автоматически. Region/function/line coverage не учитывает достаточность assertion и не доказывает поведение на отказах.

Начальная тестовая инвентаризация содержит 672 тест-функции (13 ignored). Число в CI run должно подтверждаться самим логом, а не только статусом job: отдельная risk-based traceability матрица находится в [`critical-branch-matrix.md`](critical-branch-matrix.md).

## #129 и corrective PR #155

- PR #129 head на момент аудита: `f648e1548328cb2f62792cb41181a1fac870ee60`, base — `main`; оба Rust CI успешны. Он добавляет `proptest`, replay/adversarial harness, `tests/adversarial_harness.rs`, корпус и testing docs/CI. Это готовая основа для будущих генеративных задач; повторное добавление параллельного harness приведёт к дублированию.
- PR #155 head: `b817822a86eb5e95a7cf49b9c0cd8286640df41a`, base — `codex/refactor-t21-release-closure`, то есть stacked branch, не `main`. Он меняет те же `.github/workflows/ci.yml` и тестовую инфраструктуру, но использует отличающиеся test-support структуры; на его ветке присутствуют `CODE_STYLE.md` и `TEST_QUALITY.md`, которых нет в `main`.
- Совместимость не подразумевается: сначала сравнивать базу PR #155 и corrective diff с текущим `main` и #129. Не копировать файлы вслепую и не переносить CI целиком; при интеграции брать только независимые исправления с явным разрешением конфликтов и проверкой matrix.

## Вывод baseline

Default workspace suite зелёный, полный безопасный локальный PostgreSQL набор зелёный. Вариант без default features воспроизводимо красный из-за конфигурации и разделяемого mutable env в тестах. CI пока не собирает coverage и не проверяет no-default. Основной риск качества — не недостаток общего числа unit-тестов, а отсутствие независимых моделей для state transitions/authorization и неполная проверка реальных отказов, конкуренции и негативных границ. План адресного покрытия приведён в матрице; глобальный line/region coverage не трактуется как доказательство надёжности.
