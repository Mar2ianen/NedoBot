# LLM profiles and task routes

## Цель

Заменить неявный выбор LLM через брендовые поля `Config`, эвристику имени модели и общий Gemini fallback на явную конфигурацию из трёх уровней:

```text
provider transport -> model profile -> task route
```

- **Provider** описывает transport, endpoint, имя env-переменной секрета и статические безопасные headers.
- **Model** ссылается на provider, задаёт model ID, limits и capabilities.
- **Route** задаёт упорядоченные primary/fallback model profiles для конкретной задачи.

Секреты не попадают в TOML. Заданные, но некорректные значения должны останавливать startup. Disabled features не должны требовать profile или secret.

## Capability contract

Каждая model profile явно указывает:

- `supports_images`;
- `supports_tools`;
- `supports_system_prompt`;
- `structured_output`: `json_schema`, `json_object` или `prompt_only`;
- `context_window_tokens`;
- `max_output_tokens`;
- `request_timeout_sec`;
- `thinking`: `none`, `budget` или `level_low`.

Новые capabilities добавляются только когда их использует pipeline. Нельзя выводить возможности из model ID.

## Задачи

- [x] **LP1 — schema и parser:** добавлены `src/llm/profiles.rs`, TOML example и unit tests parsing/validation без изменения live routing.
- [x] **LP2 — config loading:** `LLM_PROFILES_PATH` загружает и валидирует profiles; enabled routes проверяются на startup вместе с secret и explicit proxy-egress requirements.
- [x] **LP3 — compatibility profiles:** TOML topology задаёт явные genai adapters, endpoints, models, capabilities и egress для Gemini, Ollama Cloud, Groq, Cerebras, OpenRouter и custom OpenAI-compatible endpoint.
- [x] **LP4 — route resolver:** typed route resolver проверяет primary/fallback chain и requirements против capabilities; Gemini fallback сохранён в legacy mode, а profile mode использует route order.
- [x] **LP5 — transport profiles:** все LLM generation paths используют единый `genai` transport с direct/proxy clients, явным adapter target и safe error mapping. Policy fallback, validation retry и audit остаются в service/pipeline слоях.
- [~] **LP6 — task migration:** generation paths уже используют named routes в profile mode; native tool-call history для `ask` завершается отдельной Phase B.
- [ ] **LP7 — cleanup:** удалить legacy provider/model env routing, `LLM_SUPPORTS_IMAGES`, model-name эвристики и hard-coded fallback chain после production migration.

## Правила fallback

- Порядок моделей определяется только соответствующим route.
- Fallback по transport error, timeout, rate limit и provider unavailable разрешён по умолчанию.
- Fallback после output validation выключен по умолчанию и включается явным `fallback_on_validation_failure` конкретного route.
- Несовместимость structured output должна обрабатываться capability contract, а не распознаванием текста HTTP ошибки.
- Effective output limit — минимум из product limit и `max_output_tokens` модели; prompt builder обязан учитывать `context_window_tokens`.

## Проверки по этапам

Каждый этап: `cargo fmt`, `cargo test --all-targets`, `cargo clippy --all-targets -- -D warnings`.

Для LP1–LP4 необходимы unit tests как минимум для:

1. invalid TOML, duplicate/missing provider/model/route reference;
2. unsupported driver/capability enum;
3. invalid HTTP(S) endpoint и non-positive limits;
4. route без моделей и fallback cycle/duplicate model;
5. capability mismatch для image, tools, system prompt и structured output;
6. сохранения текущей Gemini first-comment fallback последовательности при compatibility migration.
