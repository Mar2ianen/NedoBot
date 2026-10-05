# agent-runtime

Генерик runtime агента (spec v0.1): внутренний IR, бюджет контекста,
политика инструментов, лимиты витка. К боту отношения не имеет;
бот — первый потребитель (dogfood через `/ask`).

Архитектурный инвариант:

```text
ACP != internal agent protocol
MCP != internal tool protocol
genai != internal model protocol
```

Поэтому в зависимостях крейта **нет** `genai`, `rmcp`,
`agent-client-protocol`. Адаптеры живут снаружи и переводят
внешние типы в IR этого крейта и обратно.

Состав v0.1:

- `message` — канонический `Message`/`Content`/`ToolCall`/`ToolResult`;
- `context` — `Tokenizer`, `ApproxTokenizer`, `ContextBudget`,
  `ContextPressure`, `ObservationWindow`, `truncate_chars`;
- `compaction` — `CompactionAction`/`CompactionPlan`/`CompactionPolicy`,
  жадная структурная политика с инвариантом причинных пар;
- `retained` — `RetainedFact`/`RetainedStore` (durable-состояние отдельно от transcript);
- `scheduler` — пакеты параллельных tool-вызовов без конфликтов;
- `summary` — структурный `ConversationSummary` + детерминированный рендер;
- `usage` — `Usage`/`CostEstimate` (токены ≠ деньги для subscription);
- `store` — append-first journal (`TurnRecord`/`UsageRecord`/`CompactionRecord`/`ArtifactRef`);
- `cancellation` — `TurnContext`/`CancellationToken`;
- `policy` — `Capability`, `PermissionDecision`, `ToolDescriptor`;
- `turn` — `TurnLimits`;
- `event` — `AgentEvent`;
- `error` — `AgentError` без `anyhow` через границу.
