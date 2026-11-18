# T01 — расширение общего property/replay harness

**Base:** NedoBot PR #129, head `f648e1548328cb2f62792cb41181a1fac870ee60`

**Scope:** test-only; новый harness не создавался. Использованы существующие `proptest`, `tests/adversarial_harness.rs`, corpus, seed env и shrinking fixture.

## Инварианты и пойманные дефекты

| Инвариант | Минимальный синтетический контрпример / независимый oracle | Ошибка или мутация, которую тест ловит |
|---|---|---|
| Telegram Update должен быть структурно декодируемым и его entity offsets задаются UTF-16 code units. | V2 Update с текстом из BMP и supplementary Unicode символов. `serde_json -> teloxide::types::Update` — parser oracle; границы entity независимо пересчитаны по UTF-16 и должны совпасть с Unicode scalar boundaries. | Offset, посчитанный в UTF-8 bytes или Rust `char` индексах, отклонит валидный case или выделит неверный фрагмент. V3 Update с отрицательным `update_id` обязан отклониться. |
| Один seed и один binary SHA дают одинаковый trace; shrinking оставляет минимальный дефект. | Два прогона генерируют 64 typed Update с фиксированным seed и сравнивают структурные JSON traces. Synthetic mutant `value >= 512` проверяется дважды. | Нестабильный seed/replay или изменение shrinker, оставляющее неминимальный counterexample. Ожидаемый минимум — `512`. |
| Lease generation и authorization определены независимой моделью, а порядок событий сохраняется. | Табличная role/capability reference model и synthetic последовательность `pending -> claim(generation=1) -> complete(0) -> complete(1)`. Job operation vectors сериализуются и читаются обратно без перестановки. | Если устаревший `complete(0)` меняет state либо member получает moderate/write, oracle расходится. |
| Внешний fake различает подтверждённый ответ, timeout, disconnect до ответа, disconnect после принятия и cancellation. | Typed `HttpOutcome`; без URL, тела ответа или real transport. Проверяется каждый вариант outcome и диапазон размера тела. | Свести timeout и disconnect-after-acceptance к обычному rejection, что способно вызвать duplicate side effect при retry. |
| Failure artifact не содержит raw payload, а V1 требует источник подтверждения. | В `FailureInput` передан sentinel `synthetic-secret-payload-must-not-be-logged`; сериализуется только типизированный report. Case/invariant IDs — статические строковые литералы, evidence source — enum. | Добавление raw payload в сериализуемый отчёт или маркировка неподтверждённого synthetic Update как V1. |

Синтетические Updates помечены V2: они декодируются pinned teloxide, но не заявляются как наблюдавшиеся или полностью подтверждённые по Bot API cross-field rules. Намеренно некорректный отрицательный ID — V3. Реальных V1 fixtures в этой задаче нет.

Общая authorization table в этом scaffolding — явно синтетическая fixture-модель для проверки формата actor/capability cases, а не политика NedoBot. Доменные модели доступа должны выводиться из product contract в T05/T09/T10.

Property generators для scope IDs, UTF-16 entity spans, typed Updates, actor/capability, job phase/ordered operations и HTTP outcomes запускают по 256 примеров. Успешные properties проверяют `accepted=256`, `rejected=0`; генераторы сериализуются только из ограниченных типов.

## Воспроизведение

Фиксированный seed: `72623859790382856` (`0x0102030405060708`). Повторяемая команда для typed Telegram Update:

```sh
sh scripts/replay_quality_case.sh telegram-update 72623859790382856
```

Эта команда дважды успешно запускалась; каждый запуск — `1 passed`. Shrinking fixture:

```sh
sh scripts/replay_quality_case.sh deliberate-shrink-fixture 72623859790382856
```

Результат: `1 passed`; внутри теста ошибка независимо генерируется дважды, оба shrink результата равны `512`. Неизвестный case ID выдал список поддерживаемых случаев и завершился кодом 2 до запуска Cargo. Старый CLI совместим: `sh scripts/replay_adversarial_case.sh 72623859790382856` запустил прежний URL property case.

Failure summary включает `case_id`, seed, SHA из `GITHUB_SHA`/`NEDOBOT_TEST_SHA`, replay command и proptest minimal failing input. Helper replay CLI экспортирует текущий Git SHA локально. Содержимое case ограничено созданными тестом synthetic values.

## Проверки

- `cargo fmt --all -- --check` — passed.
- `CARGO_BUILD_JOBS=3 CARGO_PROFILE_TEST_DEBUG=0 cargo test --test adversarial_harness --locked -- --nocapture` — 11 passed, 0 failed, 0 ignored.
- `CARGO_BUILD_JOBS=3 CARGO_PROFILE_TEST_DEBUG=0 cargo test --no-default-features --test adversarial_harness --locked -- --nocapture` — 11 passed, 0 failed, 0 ignored. Это отдельная target-only проверка; полный no-default workspace baseline остаётся красным на 20 ранее описанных root tests.
- `sh scripts/replay_quality_case.sh telegram-update 72623859790382856` — два запуска, каждый 1 passed.
- `sh scripts/replay_quality_case.sh deliberate-shrink-fixture 72623859790382856` — 1 passed, minimized `512` внутри теста.
- Неизвестный case ID — явный exit code 2; compatibility alias `scripts/replay_adversarial_case.sh` — 1 passed.
- PostgreSQL не нужен для T01: никакой SQL concurrency claim этим harness-тестом не доказывается; SQL lock/CAS остаются за disposable PostgreSQL lanes в T04+.

Никакие production scoring, prompts, sanctions, runtime states или schema не менялись. Live Telegram, DB, LLM, ASR и пользовательские URL не использовались.
