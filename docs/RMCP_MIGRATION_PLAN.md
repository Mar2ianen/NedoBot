# Миграция MCP на `rmcp 3`: выполненный план и инварианты

> **Статус: cutover завершён.** Legacy stdio и HTTP transports удалены; canonical `chat_db_mcp` и `nedonews_mcp_http` работают через общий `ChatMcpServer` и `ChatReadApi`. Публичный контракт опубликован отдельно от legacy API по адресу `https://nedobot.chickenkiller.com/mcp/nedonews/v2`.
>
> Исторические шаги ниже сохранены как rationale и checklist уже выполненной миграции. Исключение — upgrade `rmcp` с закреплённой `3.0.0-beta.3` на stable release: это отдельный dependency-only change, когда подходящий стабильный релиз будет выбран и проверен compatibility suite.

## Цель миграции

Заменить две самописные MCP/JSON-RPC реализации на один `ChatMcpServer`, использующий официальный Rust SDK `rmcp`:

- internal stdio transport для `/ask`;
- public Streamable HTTP transport для внешних MCP-клиентов;
- один tool router и один публичный read-model.

> Историческая цель первоначально предполагала сохранение legacy имён tools, схем и результатов. В ходе migration review это оказалось несовместимо с typed RMCP contract, поэтому был сознательно выполнен breaking cutover на отдельный `/v2` route. Legacy URL не является alias нового API; фактический контракт задают текущие RMCP schemas и golden snapshot tools/list.

## Принятые решения

### Версия SDK

Начать с актуальной `rmcp = 3.0.0-beta.3`, закрепив точную версию на время разработки. Перед production rollout перейти отдельным dependency-коммитом на `3.0.0` stable, если он уже опубликован, и повторить compatibility suite.

Не строить новую реализацию на `rmcp 2.x`, поскольку миграция разрабатывается непосредственно перед стабилизацией `3.x`.

### Публичность данных

`mcp_public` и manifest являются намеренно публичным read-model проекта. Если фактически опубликовано больше безопасных данных, чем описано в документации, нужно исправить документацию, а не автоматически урезать projection.

Сохраняются технические инварианты:

- PostgreSQL role остаётся read-only;
- MCP работает только с allowlisted views/manifest и typed use-cases;
- arbitrary SQL отсутствует;
- значения передаются SQL bind-параметрами;
- private/foreign-chat rows не попадают в публичный scope;
- секреты, токены и invite links не публикуются;
- добавление новой колонки в базовую таблицу само по себе не расширяет MCP contract.

### Один data scope для агента и внешнего MCP

Internal `/ask` и public HTTP MCP читают один публичный read-model. На текущем этапе между ними нет различий по доступным данным.

Различаться могут transport и способ использования tools:

- `/ask` запускает MCP как дочерний stdio process и использует tools в agent loop;
- внешний MCP обслуживает Streamable HTTP clients;
- tool router и `ChatReadApi` у них общие.

В будущем агент может получить отдельный memory backend. Он должен подключаться как дополнительная зависимость/набор tools поверх того же `ChatMcpServer`, не изменяя основной public read-model и не усложняя текущую миграцию заранее.

## Целевая архитектура

```text
stdio transport                 Streamable HTTP transport
       │                                  │
       └──────────────┬───────────────────┘
                      ▼
               ChatMcpServer
           rmcp ServerHandler/tool router
                      │
          ┌───────────┴───────────┐
          ▼                       ▼
   Semantic chat tools      Manifest DB tools
          └───────────┬───────────┘
                      ▼
                 ChatReadApi
                      │
                      ▼
             Public read model
        mcp_public views + manifest
```

Зависимости направлены только вниз:

- transport запускает `ChatMcpServer`;
- `ChatMcpServer` регистрирует MCP tools;
- tools валидируют MCP input и вызывают domain/read use-cases;
- `ChatReadApi` не знает о MCP, tool schemas или transport;
- SQL/read-model не знает о JSON-RPC.

## Целевая структура файлов

```text
src/features/chat_read_api/
├── mod.rs
├── service.rs              — ChatReadApi и domain use-cases
├── policy.rs               — trusted scope и общие limits
├── types.rs                — transport-agnostic DTO
├── chat.rs                 — search/context/thread
├── users.rs                — profiles/resolve/interactions
├── notes.rs                — существующие публичные notes
└── catalog/
    ├── mod.rs
    ├── manifest.rs         — load/validate manifest
    ├── queries.rs          — select/fetch/count/aggregate/search
    ├── filters.rs          — allowlisted query construction
    └── cursor.rs           — pagination

src/mcp/
├── mod.rs
├── server.rs               — ChatMcpServer + ServerHandler
├── tools/
│   ├── mod.rs
│   ├── semantic.rs         — chat.*, notes.* handlers
│   └── database.rs         — db.* handlers
├── stdio.rs                — rmcp stdio bootstrap
└── http.rs                 — Streamable HTTP service/bootstrap

src/features/ask/
├── agent.rs
└── mcp_client.rs           — rmcp child-process client

src/bin/
├── chat_db_mcp.rs
└── nedonews_mcp_http.rs
```

## Ответственности слоёв

### `ChatReadApi`

`ChatReadApi` предоставляет transport-agnostic методы:

```rust
pub struct ChatReadApi {
    pool: PgPool,
    scope: ChatReadScope,
    catalog: PublicCatalog,
}
```

Semantic use-cases:

```text
search_messages
search_messages_batch
count_messages
recent_messages
get_message
message_context
reply_thread
resolve_user
user_interactions
user_profile
list_chat_notes
list_user_notes
```

Manifest use-cases:

```text
list_tables
describe_table
select_rows
fetch_row
count_rows
aggregate_rows
search_text
```

`ChatReadApi` не возвращает `rmcp::model::*` и не принимает MCP `CallToolRequest`.

Контракт поиска общий для локального stdio-клиента `/ask` и публичного
Streamable HTTP router-а. `chat.search_messages` возвращает страницу с полями
`messages`, `total_count`, `has_more`, `next_offset` и `scan_limit_reached`; сервер ограничивает страницу 50
сообщениями. `chat.search_messages_batch` выполняет не более шести запросов и
возвращает не более пяти сообщений на запрос. `offset` позволяет запросить
следующую страницу в пределах 10000 строк; при достижении потолка `has_more=true`,
`next_offset=null`, `scan_limit_reached=true`. `chat.count_messages` использует
те же фильтры и является authoritative-путём для количества. Режимы поиска:
`hybrid` (по умолчанию), `full_text`, `any_terms`, `literal` и `whole_word`;
даты принимаются в RFC 3339 или `YYYY-MM-DD`. По умолчанию поиск исключает
ботов, строки без пользователя и автоматические пересылки; `include_forwards`
является явным opt-in и также раскрывает forwarded rows без автора. В `mcp_public` не добавляются строки или колонки и не
меняется его sanitization-контракт.

### Semantic tools

Semantic tool handlers:

- описывают MCP names/descriptions;
- получают typed input с `Deserialize + JsonSchema`;
- отклоняют unknown fields через `#[serde(deny_unknown_fields)]`;
- вызывают соответствующий метод `ChatReadApi`;
- преобразуют domain DTO в MCP `CallToolResult`;
- сопоставляют validation/domain errors с безопасными MCP errors.

### Manifest DB tools

Manifest tool handlers делают ту же адаптацию для generic public read interface:

```text
db.list_tables
db.describe_table
db.select
db.fetch_row
db.count
db.aggregate
db.search_text
```

Allowlist identifiers, SQL bindings, pagination и limits остаются внутри `chat_read_api::catalog`, а не в MCP handler.

### `ChatMcpServer`

`ChatMcpServer`:

- реализует `rmcp::ServerHandler`;
- содержит tool router;
- публикует единый `tools/list`;
- хранит `Arc<ChatReadApi>`;
- не знает, запущен он через stdio или HTTP;
- не открывает DB connection самостоятельно;
- не содержит SQL.

### Transports

Stdio и HTTP adapters отвечают только за transport lifecycle.

Stdio:

- получает готовый `ChatMcpServer`;
- запускает его поверх `rmcp` stdio transport;
- завершает service при закрытии stdin/stdout.

HTTP:

- получает factory/clone `ChatMcpServer`;
- подключает `StreamableHttpService`;
- сохраняет Origin policy, HTTP limits, tracing и graceful shutdown;
- не дублирует tools или protocol structs.

## Исторический план выполнения

### RMCP-01 — Зафиксировать текущий MCP contract — выполнено

Сделать snapshot/contract tests для обоих текущих серверов:

- `initialize`;
- `tools/list` names, descriptions, input/output schemas;
- успешные `tools/call`;
- invalid params;
- unknown tool;
- безопасные errors;
- pagination и search flags.

Добавить `#[serde(deny_unknown_fields)]` на все tool input DTO.

Отдельно сгенерировать документированный inventory `mcp_public` из manifest и синхронизировать README/TECHNICAL с фактическим публичным data surface.

Критерий готовности: изменение имени/schema/output ломает contract test.

Предлагаемые коммиты:

```text
test: lock MCP tool contracts
docs: describe public MCP data surface
```

### RMCP-02 — Подключить `rmcp 3` и transport harness — выполнено

Добавить exact dependency:

```toml
rmcp = { version = "=3.0.0-beta.3", default-features = false, features = [
    "server",
    "client",
    "macros",
    "schemars",
    "transport-io",
    "transport-child-process",
    "transport-streamable-http-server",
] }
```

Добавить минимальные tests/examples для:

- in-memory client/server;
- stdio server;
- child-process client;
- Streamable HTTP service;
- initialize/list/call/shutdown.

Production binaries пока не переключать.

Коммит:

```text
chore: add rmcp 3 transport foundation
```

### RMCP-03 — Выделить `ChatReadApi` — выполнено

Перенести из `features/ask/chat_search.rs`:

- search/recent/context/thread;
- profiles/interactions;
- domain DTO и URL helpers.

Перенести из `features/ask/chat_db_mcp.rs`:

- user resolve/fuzzy matching/transliteration;
- chat/user notes queries;
- semantic limits.

Перенести из `bin/nedonews_mcp_http.rs`:

- manifest load/validation;
- readonly pool setup;
- allowlisted generic DB operations;
- filters/order/pagination/sanitization;
- public query limits.

На этом этапе старые MCP implementations должны вызывать новый API и сохранять прежнее поведение.

Критерий готовности:

- `ChatReadApi` не зависит от `rmcp`;
- stdio/HTTP файлы не содержат SQL;
- текущие contract tests проходят без изменения snapshots.

Предлагаемые коммиты:

```text
refactor: extract shared chat read API
refactor: extract public database catalog
```

### RMCP-04 — Реализовать общий tool router — выполнено

Создать `ChatMcpServer` и два логических набора handlers:

- Semantic chat tools;
- Manifest DB tools.

Оба набора вызывают один `Arc<ChatReadApi>`.

Tool schemas должны генерироваться из typed input через `rmcp`/`schemars`. Удалить ручное дублирование JSON Schema после подтверждения snapshot parity.

Критерий готовности:

- один `ChatMcpServer` можно запустить через любой transport;
- `tools/list` не зависит от transport;
- ни один tool handler не содержит SQL;
- старые tool names и outputs сохранены.

Коммит:

```text
refactor: define shared rmcp tool router
```

### RMCP-05 — Перевести internal stdio server — выполнено

Временно добавить `chat_db_mcp_rmcp`, чтобы можно было провести canary через существующий `runtime.ask_db_mcp_command`.

Binary должен:

1. строго прочитать необходимый config;
2. создать readonly pool и `ChatReadApi`;
3. создать `ChatMcpServer`;
4. запустить его через rmcp stdio transport.

Удалить `dotenvy::dotenv()` из child binary: `env_clear()` родителя должен оставаться реальной границей окружения.

Добавить real child-process integration test.

Коммит:

```text
feat: serve chat tools through rmcp stdio
```

### RMCP-06 — Перевести `/ask` client на `rmcp` — выполнено

Заменить ручной line-based `McpClient` в `agent.rs` на отдельный `features/ask/mcp_client.rs`, использующий `rmcp` child-process transport.

Сохранить:

- command/args/env config;
- `env_clear()` и env allowlist;
- per-call timeout;
- kill child on completion/drop;
- bounded result size;
- tool-call audit;
- `TOOL_ERROR` fallback.

После initialize получать реальный `tools/list` от `ChatMcpServer` и строить из него каталог для LLM. Удалить ручную копию MCP tool catalog из `agent.rs`.

`notes.add_user`, web и GitHub остаются отдельными agent actions: они не являются read-only tools текущего `ChatMcpServer`.

Коммит:

```text
refactor: use rmcp client for ask tools
```

### RMCP-07 — Internal canary и cleanup — выполнено

Production rollout:

1. задеплоить старый и новый stdio binaries;
2. переключить `runtime.ask_db_mcp_command` на `chat_db_mcp_rmcp` в profile TOML;
3. проверить search/context/resolve/notes/multi-step `/ask`;
4. проверить `ask_runs` и `ask_tool_calls`;
5. проверить отсутствие orphan child processes;
6. после стабильного canary удалить legacy stdio protocol/client;
7. вернуть новому binary имя `chat_db_mcp`.

Rollback: вернуть старое значение `runtime.ask_db_mcp_command` и рестартовать bot.

Коммит cleanup:

```text
refactor: remove legacy stdio MCP transport
```

### RMCP-08 — Перевести public HTTP server — выполнено

Подключить тот же `ChatMcpServer` к `rmcp` Streamable HTTP service.

Сохранить:

- `MCP_BIND` и `MCP_PATH`;
- внешний URL;
- Origin allowlist;
- Nginx rate/body/timeout limits;
- restricted PostgreSQL role;
- startup manifest validation;
- static avatar serving;
- безопасное логирование без request body/filter values;
- graceful shutdown.

HTTP adapter не должен иметь собственный список tools или protocol version.

Коммит:

```text
feat: serve public MCP through rmcp HTTP
```

### RMCP-09 — Единый compatibility suite — выполнено

Один набор тестов запускать против stdio и Streamable HTTP transports:

```text
initialize
tools/list
tools/call
unknown tool
invalid params
unknown fields
search modes
pagination
select/filter/order
count/aggregate
message context
user resolve
notes
DB timeout
safe error
transport shutdown
```

PostgreSQL integration дополнительно проверяет:

- discussion data доступна;
- private/foreign chat rows отсутствуют;
- readonly role не выполняет write/DDL;
- manifest соответствует реальным views;
- stdio и HTTP возвращают одинаковые domain results.

Коммит:

```text
test: verify rmcp transport parity
```

### RMCP-10 — Public sidecar rollout — выполнено

Запустить новый сервер рядом со старым:

```text
legacy: 127.0.0.1:8787
rmcp:   127.0.0.1:8788
```

Проверить новый через:

- официальный rmcp client;
- MCP Inspector;
- текущего внешнего клиента;
- реальные search/select/aggregate calls;
- pagination и errors.

После smoke переключить только Nginx upstream `8787 -> 8788`, сохранив внешний URL. Старый server оставить запущенным на период наблюдения.

Rollback: вернуть upstream на `8787`.

### RMCP-11 — Перейти на `rmcp 3.0` stable — отложено до отдельного dependency-only change

Перед финальным production cutover проверить crates.io. Если stable опубликован:

```toml
rmcp = { version = "=3.0.0", ... }
```

Сделать dependency-only коммит и повторить полный compatibility suite для обоих transports.

Коммит:

```text
chore: upgrade rmcp to 3.0 stable
```

### RMCP-12 — Удалить legacy HTTP и обновить документацию — выполнено

После стабильной эксплуатации удалить:

- ручные JSON-RPC request/response/error types;
- ручной initialize/list/call dispatch;
- ручные protocol versions;
- старый HTTP handler;
- временные sidecar binary/unit;
- дублированные tool schemas/catalogs.

Обновить:

- `AGENTS.md`;
- `README.md`;
- `docs/TECHNICAL.md`;
- `docs/AUDIT_2026-07-26.md`;
- operations/deploy smoke checklist.

Зафиксировать публичность read-model, фактический manifest inventory, общую архитектуру tools/API и rollback procedure.

Коммиты:

```text
refactor: remove legacy public MCP transport
docs: document public rmcp architecture
```

## Будущее расширение памяти агента

Отдельная agent memory не входит в текущую миграцию. Когда она понадобится, целевая форма:

```text
ChatMcpServer
├── Semantic chat tools ───────> ChatReadApi/public read model
├── Manifest DB tools ─────────> ChatReadApi/public read model
└── Agent memory tools ────────> AgentMemoryApi/separate backend
```

Memory tools могут включаться только в stdio instance через конфигурацию tool router/factory, не создавая второй `ChatReadApi` и не меняя permissions существующих public data tools.

До появления требований к memory не добавлять пустые traits, feature flags или storage abstractions.

## Последовательность реализации

### Фаза 1 — контракт и foundation

```text
RMCP-01 contract snapshots и public inventory
RMCP-02 rmcp 3 dependency и transport harness
RMCP-03 ChatReadApi/catalog extraction
RMCP-04 общий ChatMcpServer/tool router
```

### Фаза 2 — internal stdio

```text
RMCP-05 rmcp stdio server
RMCP-06 rmcp child-process client
RMCP-07 production canary и legacy cleanup
```

### Фаза 3 — public HTTP

```text
RMCP-08 Streamable HTTP adapter
RMCP-09 transport parity suite
RMCP-10 sidecar rollout
RMCP-11 rmcp 3 stable upgrade
RMCP-12 legacy cleanup и документация
```

## Обязательные проверки

Для каждого нетривиального этапа:

```bash
cargo fmt --all
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
./scripts/test.sh
```

Для transport этапов дополнительно обязательны:

- real child-process stdio integration test;
- real local Streamable HTTP integration test;
- MCP Inspector smoke;
- sidecar production smoke до переключения Nginx.

## Не смешивать с миграцией

До достижения transport parity не менять в тех же коммитах:

- бизнес-семантику поиска;
- tool names;
- pagination contract;
- состав public views;
- новые write tools;
- `/ask` authorization;
- Nginx limits;
- LLM prompt;
- отдельную agent memory.

Первый рабочий блок: `RMCP-01 -> RMCP-02 -> RMCP-03`. После него `rmcp` tools станут тонкими adapters над готовым `ChatReadApi`, а оба transport-а смогут использовать один `ChatMcpServer` без дублирования protocol и domain logic.
