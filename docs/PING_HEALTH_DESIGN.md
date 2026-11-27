# RFC: /ping as NedoBot runtime health snapshot

Status: proposal (documentation only). Baseline: main @ 45560c791b9ded3daff9d6bc67db479c5342fc1c, 2026-11-25.
Scope: Telegram bot process, background workers, PostgreSQL, /ask agent, agent-side MCP, public RMCP HTTP, and optional external dependencies.

## 1. Goal and non-goals

Replace the literal pong with a cheap, useful, privacy-safe health snapshot. The primary question is not merely "is the process alive?" but "is the bot making forward progress in its configured work?". Separate the availability of the Telegram ingress, persistence, workers, and agent/tool systems.

Non-goals: a full Prometheus replacement; arbitrary SQL/URL execution from Telegram; launching LLM/ASR inference for monitoring; reproducing historical job reports on each invocation; exposing diagnostic details to public chats; claiming that no traffic means a broken service.

All thresholds below are **initial recommendations** to tune against real production distributions, not verified SLOs.

## 2. Existing code: inventory and evidence

| Existing component | Source | Reuse / gap |
| --- | --- | --- |
| /ping | src/telegram/command_handler/read_only.rs; src/telegram/commands.rs | Ping is a unit variant and returns pong. Changing to Ping(String) allows optional args. |
| /db | src/telegram/command_handler/read_only.rs | Existing select 1::bigint through AppState.pool; avoid duplicate logic and bounded acquire. |
| Main PgPool | src/db/mod.rs | max_connections(5); do not fan out heavy SQL on the command path. |
| AppState | src/state.rs | Pool, config, ask_slots semaphore, AskDeliveryMetrics, optional URL feed snapshot / spam reputation store; no unified HealthState. |
| Worker lifecycle | src/features/jobs/worker.rs; src/main.rs | WorkerSupervisor; 18 named workers plus an Unknown sentinel (19 WorkerId variants), active workers depend on feature/runtime gates. Poll outcomes and shutdown handling exist, but no shared per-worker heartbeat registry. Unexpected exits are fatal and trigger shutdown. |
| SQL queue observability | src/features/jobs/observability.rs | Already reports first-comments, embeddings, post-history, reviews: status counts, oldest ready/processing ages, expired leases, reclaim counts and sanitized error groups. It executes multiple SQL statements: use cached sampling, not per-request full report. |
| Ask concurrency and audit | src/state.rs; src/features/ask/repo.rs; src/features/ask/types.rs | ask_slots; ask_runs with running/completed/failed, typed error_kind, delivery status, provider/model, timestamps; ask_tool_calls with status/latency_ms/error_kind. Keep aggregates bounded in time. |
| Ask delivery safety | src/features/ask_metrics.rs | Process-local unknown_delivery_failures counter; currently cumulative only. |
| Agent-side chat MCP | src/features/ask/mcp_client.rs; src/features/ask/agent.rs | /ask starts a **new stdio child process** with per-chat scoped config, RMCP initialize and tools/list, then calls tools; timeout and tool failure handling exist. Does not share the public HTTP MCP process. |
| External search MCP | src/features/search/mcp.rs | Independently spawns search/GitHub MCP subprocesses for requested search; report these separately from chat-DB MCP. |
| Public RMCP HTTP | src/bin/nedonews_mcp_http.rs; src/mcp/rmcp_http.rs; src/mcp/bootstrap.rs | **Separate binary/process**, independent readonly pool and Streamable HTTP listener, hosts/origins guard, request timeout; cannot be called healthy solely because the bot is healthy. |
| QR and URL shadow | src/features/qr_url.rs | Separate QrEvidence/QrShadowRefresh workers; QR decoding has bounded timeout. |
| Voice | src/features/voice/pipeline.rs; src/features/voice/repo.rs | voice_transcription_jobs with phases, retry_wait, failed, delivery_unknown; skipped/no-speech is not an outage. |
| Moderation | src/features/new_user_audit/service.rs; src/features/decision_audit/service.rs | Queue-based new-user audit/decision audit; moderation must be monitored even if /ask works. |
| Build and optional features | Cargo.toml; src/main.rs | Package v0.12.0, default features auto-comment/ask/voice/public-mcp/moderation/spam-sync/manual-moderation, but runtime config still determines which workers are active. |

Separate production instances should show their **own** health, keyed internally by validated community.instance.id. Never aggregate two instances by assuming that the global Telegram chat or the MCP endpoint implies a single process.

## 3. User-facing contract and authorization

**Every /ping sends one real Telegram message immediately, then updates that same message as fresh observations arrive.** This applies to private chats **and** groups. Never use the native ephemeral Telegram draft for /ping.

- **/ping**: emit the first *real* message immediately using the latest cached snapshot, with explicit CHECKING on refreshable components; do not wait for DB, MCP, or other probes before the first send. Once delivered and its message_id is known, kick or join the shared, bounded cheap health refresh and stream edits to that message. No user-input-driven heavy MCP or LLM probes. Show overall verdict, uptime, build, DB, worker backlog summary and agent/MCP as generic states only.
- **/ping full**: identical real-message-first / edit-in-place protocol, but expanded detail **only for configured owner_telegram_id in a private chat**: individual queues, last success/failure ages, slots, MCP components, freshness and sanitized error categories. A chat admin, sender_chat, and /ask permission do not grant full diagnostics.
- **/ping probe**: owner-only private chat; same initial real message + edits; opt-in rate-limited active read-only probes with a global deadline. No model inference or mutating tool calls. Other arguments return concise help, without probes.
- **/status** already provides chat statistics; do not repurpose it. Retain **/db** as an existing compatibility command.
- Render with escaped HTML and typed states; enforce message-size cap. No credential, host/IP, DSN, SQL, scope, username/user ID, question, tool arguments, prompt or stacktrace in public output. Full view exposes only safe error categories.

An illustrative public message immediately after /ping (sample values, **not actual production data**):

    NedoBot 0.12.0 · 45560c79
    Health: CHECKING · uptime 04:32
    PostgreSQL: CHECKING (last OK 17 s ago)
    Workers: 15 active, 1 slow (cached)
    Queues: refreshing…
    Agent: idle · MCP(chat): last OK 21 min ago
    Public MCP HTTP: UNKNOWN (no fresh monitor)

Then the **same message** is edited (no second message):

    NedoBot 0.12.0 · 45560c79
    Health: DEGRADED · uptime 04:32
    PostgreSQL: OK · 8 ms
    Workers: 15 active, 1 slow
    Queues: embeddings 32 ready (oldest 14 min)
    Agent: idle · MCP(chat): last OK 21 min ago
    Public MCP HTTP: UNKNOWN (no fresh monitor)
    Checked: 0.7s · completed

An illustrative owner-only /ping full excerpt:

    /ask: active=1/4, finished(1h)=8, failed(1h)=1
    MCP chat stdio: init OK, tools/list OK, call errors(1h)=0
    MCP search/github: IDLE / DISABLED
    MCP public HTTP: remote canary OK @ 20:13 UTC
    first-comments: ready=0, expired_leases=0
    embeddings: ready=32, oldest_ready=14m, expired_leases=0
    voice: ready=0, delivery_unknown(24h)=1
    decision audit: ready=2, oldest_ready=40s
    Worker qr_evidence: idle, last_poll=3s, failures=0

No example numbers may be hardcoded into runtime output. Rendering must distinguish fresh, cached, stale, disabled, and timed-out observations.

## 4. Typed health model

Use typed internal fields, not strings assembled inside handlers:

    enum CheckState {
        Healthy,
        Degraded,
        Unavailable,
        Unknown,
        Disabled,
    }

    struct CheckSnapshot {
        id: &'static str,
        state: CheckState,
        checked_at: Option<SystemTime>,
        observation_age: Option<Duration>,
        elapsed: Option<Duration>,
        detail: SafeDiagnostic,
    }

    struct HealthSnapshot {
        generated_at: SystemTime,
        uptime: Duration,
        build: BuildIdentity,
        components: Vec<CheckSnapshot>,
        workers: Vec<WorkerSnapshot>,
        queues: Vec<QueueSnapshot>,
    }

Optional checks absent by feature flag or runtime configuration are **Disabled**, not failed. No observations or stale observations are **Unknown** (with age); they are never silently converted into Healthy. A worker actively running a legitimate long job is not automatically Unavailable because it has not completed a poll recently.

Overall state should be based only on *configured critical components*: core process, DB, Telegram ingress/delivery signal, enabled critical worker progress, and sustained critical queue stall. Optional provider/search outages usually degrade an affected feature, not the whole bot. Unknown should be surfaced but not silently counted as success. A single transient failure should not flap global status: use consecutive samples or hysteresis.

Separate statuses for availability, activity, and completeness of observation. Return the last snapshot even on partial probe failures, marking the failed component and collection age.

## 5. Data collection design

### A. In-process data for initial /ping frame (no I/O before first send)

Create src/features/health/{mod.rs,model.rs,registry.rs,render.rs,collector.rs}. Add Arc<HealthRegistry> to AppState, initialized with process start Instant + build identity. For every active WorkerId record registered_at, last_poll_started_at, last_poll_finished_at, last_success_at, last_error_at, consecutive_errors, processed/idle/failed counts, and current poll state. Register workers according to **actual spawned instances**, not every compile-time WorkerId.

Instrument run_poll_worker, run_interval_worker, run_fixed_delay_worker at poll boundaries and any custom worker loops (profile refresh queue, etc.). Record failures only with static/bounded categories; do not store error chains or user material. Keep the supervisor's existing shutdown semantics unchanged. Track health during shutdown as Draining rather than reporting a bogus green.

Instrument inbound/update processing counters at relevant router boundaries; avoid double-counting command ingress vs persistence. Count accepted, persisted, rejected/skipped, errors, last update timestamp, and optional rolling durations. A quiet chat is Idle/Unknown as appropriate, not "ingress broken". A successful /ping Telegram send cannot have its actual RTT embedded in the same response unless an edit is made; prefer the **last observed** delivery or periodic getMe probe (do not conflate "message received" with successful outbound API access).

At /ask admission/completion measure current active slots (configured capacity - ask_slots.available_permits), attempted, finished, failure kind, timeouts, duration, last success and last error. Add a rolling window/ring-buffer or monotonic cumulative counters with windowed deltas; never calculate "last hour" from a raw process-cumulative integer. Reuse AskDeliveryMetrics but mark existing value cumulative unless a window is implemented.

### B. Read-only periodic DB sampler

Small bounded aggregate queries, sampled every 30-60 seconds with a hard per-query and global deadline, and cached as Arc<snapshot>. A single collector with singleflight avoids 20 concurrent /ping commands launching 20 queries. Do not hold a pool connection while waiting on an external request. Respect PgPool max_connections(5), use pool.size() and pool.num_idle() where supported, and cap sampler pressure; no unbounded concurrent acquires. Database liveness is a bounded SELECT 1, with response latency and last success time.

Reuse the shape and safe error normalization of jobs/observability.rs, but do not invoke the entire job_lifecycle_report from each command. Query pending/ready/processing counts, oldest **due** ready age, expired leases, rolling terminal failures and last successful completion. Historical accumulated failed count or lease reclaims are not an incident by themselves. Use instance/chat scope WHERE supported by schema and actual deployment semantics.

Phase 1 cached queues: post_comment_jobs, telegram_message_embeddings_gemma2, post_history_entries, spam_review_requests (existing observability); extend later to:
- voice_transcription_jobs: separate running phases, retry_wait, failed, sent, and delivery_unknown;
- telegram_qr_scan_jobs and URL shadow refresh queue: separate pipelines;
- new_user_audit_jobs plus materialization progress;
- moderation_decision_jobs, decision action queue, CAPTCHA lifecycle, reports delivery;
- profile refresh intents/queue and spammer-avatar embeddings.

Do not assume all these tables have identical status/lease columns. Validate each query against existing migrations and indexes before shipping. For high-volume tables, precompute or add narrow partial indexes where needed; do not add indexes merely to support an optional /ping until EXPLAIN ANALYZE has been reviewed.

### C. Agent, LLM, and MCP (three distinct levels)

**Agent execution:** /ask active/limit, request latency p50/p95 only when sample count suffices, completed/failed by typed AskFailureKind, stale running runs (distinguish live in-process work from DB records left behind after restart), delivery_unknown, successful tool call ratio and last-success ages. Persisted ask_runs and ask_tool_calls support retrospective samples; in-process metrics cover currently active work.

**Agent chat-DB MCP stdio:** instrument McpClient::start_for_scope after child spawn, RMCP initialize, tools/list, required-tool/allowlist validation, tool_call success/failure/timeout and shutdown. Counters should track startup latency, tools/list latency, call latency, and generic error kinds, NOT tool arguments/results or child command/env. Record separate snapshots for each lifecycle step, with bounded cardinality. A recent successful /ask is evidence; no /ask traffic means Idle/Unknown, **not failure**. An owner-only /ping probe may launch exactly one isolated, bounded, read-only MCP child in a configured chat scope (never user-supplied scope) and immediately shut it down. Do not spawn children on public /ping.

**Search/GitHub MCP:** this is a different subprocess lifecycle (features/search/mcp.rs). Instrument actual operation attempts and surface independent status; Disabled if the corresponding config/agent allowlist disables it. Do not issue paid or external searches for a health check.

**Public RMCP HTTP:** standalone binary, different readonly DB pool and protocol endpoint. The main bot must not infer availability from its own pool or from its chat-DB MCP. Two independent signals:
1. local process RMCP initialization, tools/list and one tiny in-scope read from a separate, fixed service-side probe;
2. optional external end-to-end canary through the fixed, explicitly configured public HTTPS endpoint (proxy/TLS/Origin/Host path), run by trusted monitor or periodic background sampler.
Without an authorized probe result, state is Unknown, not Healthy. MCP catalog-only success is not DB-read success. Never enable a blanket unauthenticated health endpoint which leaks internals; do not alter MCP ACLs or expose arbitrary tooling.

**LLM, RAG, ASR, other providers:** derive state passively from real operations, track circuit/outcome categories per logical service, including 429/timeouts/5xx vs validation failures. If no traffic, show Idle/Unknown. No synthetic inference, transcription, token usage, or billable calls from /ping.

### D. Health of data flow

Measure queue readiness **and** progress. Worker heartbeat alone proves its loop is alive but not that work can finish. Queue metrics alone may be empty when traffic is absent. For each enabled pipeline combine: ready jobs + oldest due age + last finished job + consecutive failures + expired leases. A disabled queue is not an incident. A legitimate backoff before next_attempt_at is not overdue. Treat delivery_unknown specially (manual review needed; do not retry blindly).

For moderation, higher operational priority belongs to new-user audit, decision audit and CAPTCHA; delays can directly affect safety of the community. QR and URL shadow must be separate: successful QR extraction does not guarantee the later URL materialization succeeded.

## 6. Initial thresholds and sampling (proposal)

| Signal | Collect | Default degraded | Default unavailable |
| --- | --- | --- | --- |
| DB SELECT 1 | every 30s, <=1s deadline | 2 consecutive failed samples | 3 consecutive failed samples |
| Worker registry | in-process on every poll | active poll exceeds 2x configured max duration or 3 consecutive failures, with grace | unexpected exit (already fatal) or sustained stalled critical worker |
| First-comment due backlog | 60s | oldest due >2m | >10m if worker enabled and progress absent |
| Moderation decision/audit due backlog | 30s | oldest due >1m | >5m sustained |
| QR/URL-shadow due backlog | 60s | oldest due >5m | >20m sustained |
| Embedding due backlog | 60s | oldest due >10m | >60m sustained |
| Voice due backlog | 60s | oldest due >10m | >30m sustained |
| Agent chat MCP initialization | passive per /ask; optional admin probe | repeated startup/tools/list failures | sustained unavailability for enabled /ask |
| Public MCP HTTP | separate monitor every 1-5m | repeated failed/stale probes | sustained failed external end-to-end probe |

Thresholds are starting points; avoid cross-feature uniform limits. Keep 2-3 sample hysteresis for global transitions. For queues driven by sporadic input, "no completed jobs" with empty queue is normal. If a probe's cached TTL expires, state becomes Unknown (stale), not Healthy.

**Budget:** /ping initial-frame preparation should perform zero external/network/SQL operations and finish snapshot rendering quickly. Only after the real message is acknowledged may the shared cheap refresh run and publish further edits. Background sampling should be bounded and cancellable, no unbounded retry loops, no sampling every worker poll. /ping probe is rate-limited, non-concurrent (singleflight), owner-only and opt-in.

## 6.1. Strict deadlines and fail-closed behavior (required)

**Invariant:** a check that fails to answer before its deadline is not successful. Never block the **first** real Telegram message on any probe, never interpret timeout as Healthy, and never create a second message as a fallback after an ambiguous send.

| Operation | Suggested hard wall-clock deadline | Outcome |
| --- | --- | --- |
| Initial /ping message composition (memory/cache only) | 100 ms | Still attempt to send a minimal safe initial message |
| Initial real Telegram send | 2 s request timeout (operational default) | Record unknown delivery if ambiguous; stop edits without a confirmed message_id; do not resend |
| /ping cheap refresh after initial send | 5 s overall, shared/singleflight | Finish with available data, mark pending checks TIMEOUT, edit same message |
| DB collector sample | 1 s for SELECT 1; 3 s for queue batch | Report TIMEOUT in this run, retain previous sample with age |
| Public MCP HTTP/background canary | 3 s | Record timeout; global degradation only per policy/hysteresis |
| /ping probe owner-only | 5 s global probe deadline, individual sub-deadlines <=2 s | Stop unfinished checks, final edit contains TIMEOUT rows |
| Each edit / final edit | Bounded request timeout (e.g. 2 s), rate-limited | On failure retain the previously delivered real message; no new message |

These are initial budgets to tune using production telemetry. The global check deadline and UI request deadlines are **different concerns**: after probes time out, permit a bounded final edit attempt; it must not restart probes or push the check deadline forward. Enforce one global deadline with per-component sub-deadlines (timeout_at and remaining-budget minimum). Skipped samples must be marked stale/Unknown.

Never have each /ping independently start a full database report: **one shared refresh task**, singleflight, cache TTL/rate-limit, and fan-out of its snapshots to each active message. Additional /ping calls get their own real message immediately, then subscribe to an existing refresh or receive cached results. Do not queue arbitrary numbers of refresh requests.

For owner-only active MCP checks, stop/cancel unfinished futures, kill the scoped MCP child on cancellation using existing kill_on_drop ownership, and avoid holding PgPool connections during external probes. If the bot cannot contact Telegram at all, it cannot deliver a report; out-of-band monitoring must detect that. A send timeout can be an ambiguous delivery, and retrying a POST risks duplicate messages; no speculative resend.

Status after deadline must not remain CHECKING forever if the final edit reaches Telegram. If the final edit cannot be delivered, emit a separate internal metric/log event (sanitized) so external monitoring knows the visible message might be stale. Do not mark probe success from a queued DraftSink update.

**Required tests:** never-resolving database future, MCP initialization hang, HTTP hang, initial-send timeout/unknown delivery with zero follow-up sends, Telegram edit timeout, shared collector already active, 100 simultaneous /ping callers, late data after global timeout, and correct final message_id reuse.

## 6.2. Real-message-first progressive editing with teloxide Drafter (required)

### Verified current fork behavior

- NedoBot uses Drafter::snapshots / DraftSink::update / Drafter::finish in src/telegram/command_handler.rs and a shared AppState.drafter_limiter.
- src/telegram/ask_drafter.rs uses NativeRichBackend only for private /ask; RichEditInPlaceBackend is used for /ask groups.
- The pinned teloxide fork (rev ce840d82daf47c3e9b9ee16fb05d62fbe0c24727, crates/teloxide/src/drafter/telegram.rs) has the exact behavior needed:
  - RichEditInPlaceBackend::update with message_id=None **sends a real rich message**, stores the returned message_id;
  - subsequent updates **edit** the existing message via edit_rich_text;
  - finish **edits the same message** if message_id is known;
  - EditAbortPolicy::KeepPreview retains the existing message on cancellation.
- Fork defaults: DraftConfig coalesce_window=75ms, min_update_interval=1s, request_timeout=8s, terminal_timeout=30s. Set /ping-specific bounded values; never inherit defaults that violate the health deadline.

### Correct lifecycle

1. Authorize command and select **public or owner-only** renderer before doing any work.
2. Build the initial frame from local registry and cached samples, plus CHECKING placeholders; do not await DB/MCP/HTTP.
3. Create a **RichEditInPlaceBackend for every chat kind** (including DMs) with reply_parameters and EditAbortPolicy::KeepPreview. No NativeRichBackend for /ping.
4. Create Drafter::snapshots using the shared rate limiter and a /ping-specific config; queue first frame with DraftSink::update, **immediately flush with a bounded timeout** to send a real message. Set a tiny valid coalesce_window (e.g. 1ms). The initial network send is async, so "immediately" means no dependency checks before it, not zero network latency.
5. Only after first send is acknowledged and the backend has a known message_id, start or subscribe to a shared bounded refresh. If the initial send fails/has unknown delivery, **stop**; do not call finish on a backend with unknown message_id because that may send a duplicate new message.
6. As individual cheap checks complete, produce typed ProbeProgress events in one aggregator, regenerate the **entire** message, and issue DraftSink::update. Out-of-order or stale events must not roll back terminal states. Drafter coalesces latest-wins updates and applies the shared rate limiter. There is no promise that every intermediate frame will be visible.
7. On completion/deadline, label unresolved checks TIMEOUT and call Drafter::finish exactly once to **edit** that existing message with the final state. If edit fails, record diagnostic failure and leave the real message as-is; never send a replacement.

### Limits and safety

- The **first send is mandatory** for every /ping; a cached snapshot is immediately visible as a real reply even if every probe hangs.
- A plain /ping may refresh the cheap DB and queue snapshot through shared singleflight collector, not launch MCP subprocesses or billable external service calls. Show last measured MCP state and its age. /ping probe adds explicitly authorized active checks; /ping full only broadens visibility.
- Do not clone /ask's private-chat native-draft behavior or its DeletePreviewBestEffort abort policy. KeepPreview prevents removal of a real status message.
- RichEditInPlaceBackend currently has **no public way to attach an already sent message_id**: avoid a separate manual Bot::send_message followed by creating a fresh Drafter; that combination would risk a second message. Let the first Drafter update perform the real send and obtain the message_id.
- The backend itself handles send->edit transition. Keep /ping's code feature-independent of AskDrafterBackend (currently behind feature=ask); use RichEditInPlaceBackend directly so --no-default-features can compile.
- Initial send, edit/finalization and probe deadlines must be tested independently. Do not let a stuck Telegram send launch new work indefinitely. Do not attempt a second send when an API timeout makes delivery ambiguous.
- Use a bounded update interval (default initial recommendation ~1s) and cap total edit volume. Do not leak owner-only diagnostics to group messages. Renderers escape HTML and keep all frames within Telegram limits.
- Optional DrafterObserver / DrafterMetricsCollector events can report preview/finish latency and safe error classes, without logging payloads.

### Additional acceptance tests

- DM and group both send **one real first message**, not native drafts; every subsequent event uses the **same message_id** via editMessageRichText.
- First frame appears before any mocked DB/MCP future can resolve; delayed/hanging probes do not delay it.
- First send timeout/ambiguous delivery **never** triggers finish/send fallback. Refuse edits without a confirmed message_id.
- Update coalescing and out-of-order events preserve the latest monotonic state; 100 simultaneous requests do not produce 100 expensive refreshes.
- Completion and global deadline both trigger one final **edit**, not a second message.
- Cancellation leaves the real message (KeepPreview), owner/group permissions hold across every frame, and --no-default-features builds.

## 7. Implementation steps (small, reviewable PRs)

1. **P1 model/registry + actual-message-first UI:** health model; process uptime/build; worker heartbeat; /ping, /ping full owner auth; RichEditInPlaceBackend first update sends a real message, subsequent updates edit the same ID. Preserve /db. No schema migrations.
2. **P2 cheap collector + progressive edits:** DB SELECT 1 and cached four existing queue projections; singleflight fresh refresh after the initial send, stream typed result events into Drafter updates, enforce deadline and one final edit. Add windowed counters and instance scope.
3. **P3 /ask and MCP:** instrument ask slots, lifecycle, active requests; MCP child init/list/call durations/errors; LLM attempts; error and delivery windows. Keep child lifecycle unchanged.
4. **P4 extended queues:** QR/shadow, voice, moderation, captcha, profiles, report delivery. Verify each query/index and feature gate.
5. **P5 public MCP monitor:** separate isolated HTTP canary or dedicated monitor interface; public /ping exposes only sanitized aggregate. Optional owner-only explicit probe.
6. **P6 ops:** optional metrics export and external dashboard/alerts reusing the same registry; no second metrics truth source.

Do not implement all queues in one patch; preserve release gates and rollback-friendly commits.

## 8. Acceptance tests

- Command parsing: /ping, /ping full, /ping probe, /ping@Bot full, unknown arguments; /status day/week/month unchanged; /db unchanged; compile with default and --no-default-features.
- Permissions: owner in private chat sees full; stranger/private and owner/group do **not** receive full diagnostics; sender_chat/forwarded authors cannot impersonate owner; HTML escaping for all rendered fields.
- No-traffic startup: first real message delivered immediately; no fake Healthy or failure for /ask/MCP/LLM; Disabled feature gates; stale sample becomes Unknown.
- Health model: deterministic verdict precedence, toggled criticality, hysteresis, partial collector failure, no unchecked conversion of cumulative counters into per-hour values.
- Worker fixtures: Processed/Idle/Failed transitions, long in-flight work, stuck poll, restart reset, unexpected exit, graceful drain, no heartbeat from disabled worker.
- PostgreSQL integration: query returns verified scoped results, empty and delayed queues, expired leases, retry_wait in future, SQL timeout, lost DB, sampler limited parallelism. Ensure /ping still renders cached degraded state if DB unavailable.
- MCP tests using fake RMCP child/server: init success, tools/list incomplete, tool timeout, absent required tool, spawn failure, no requests, shutdown, distinct public HTTP failure; ensure no child spawn on public /ping.
- Load test: 100 concurrent public /ping calls each get one real response (and edits to its own ID), but share at most one DB refresh; no extra subprocesses, bounded allocations/response bytes, no secrets, tokens, raw user text or internal errors in rendering or logs.
- Delivery observability: 'delivery_unknown' distinguished from provider failure; no probing mutates user/chat state.
- Rollout: compile/test and SQL integration first, then feature-enabled canary; compare snapshot to CLI job_lifecycle_report; verify one production process and public MCP separately. Roll back by reverting feature/handler, no DB migration needed in first phases.

## 9. Key decisions

- Use **/ping**, not /status. Every /ping sends a real Telegram message first and then edits that same message with fresher data; never use ephemeral native draft and never send a final duplicate.
- Public view cheap and safe; detailed view owner-only DM.
- Initial snapshot based on existing AppState and worker supervision; fresh cheap checks run only after the initial send using bounded singleflight collection, never heavyweight synchronous work before the first reply.
- Agent chat-DB MCP, search/GitHub MCP, and public RMCP HTTP are **three different monitoring subjects**.
- No operations that incur LLM/ASR costs, no SQL or tools supplied by Telegram users.
- Stale, disabled, idle, and failed are distinct concepts.
- Queue progress + age matters more than raw uptime or static success counters.

This RFC is a design document only; it does not claim that any proposed HealthRegistry, probe or /ping full implementation currently exists.
