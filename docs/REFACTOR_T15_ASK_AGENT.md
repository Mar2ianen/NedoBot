# T15 — Ask agent/control-flow decomposition

## Scope

Baseline SHA: `e3363fe7061f562092864e67fa9d34452ab01328` (T14 CI-success checkpoint). Branch: `codex/refactor-t15-ask-agent-decomposition`, stacked on T14 PR #147. No public API, schema, migration, or dependency changes.

## Changes

- `agent.rs` remains the ask-turn orchestrator; provider turn generation and timeout retry policy are isolated behind `TurnRunner` in `agent/turn_runner.rs`.
- Native tool policy, catalog construction, audit projection, dispatch, and execution are grouped in `agent/tool_executor.rs`. The existing authorized-tool dispatch now enters through `ToolCallExecutor`.
- Message/source provenance, verified count scopes, reply ancestry filtering, and bounded observations live in `agent/evidence.rs` as `EvidenceTracker` and `ScopeTracker` state.
- Prompt construction and native tool-history compaction are in `agent/context.rs`, with `ContextBuilder` as the orchestration entry point.
- Added replay characterizations for partial tool completion (stable call identity) and duplicate note-write suppression (same authorized note call is served from the per-turn cache).
- Existing policy behavior and serialized/native tool history are unchanged; no generic agent framework or secret-bearing test seam was introduced.

## Verification

- `cargo fmt --all -- --check` — passed.
- `cargo check --lib --locked` — passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `cargo test --lib --locked features::ask::agent::tests` — 29 passed, 0 failed, 1 ignored, 590 filtered.
- The required `cargo test --workspace --all-targets --locked` was started, but local compilation was interrupted after roughly 11 minutes while the shared host load average was about 282. No test failure was reported; this run is inconclusive. CI must provide the full-workspace result.

## Invariants and limitations

- Reply-branch filters and verified-count scope checks are unchanged and covered by the existing ask tests.
- Observed message IDs, source URL alias order, mutation opt-in, and tool allowlist enforcement remain in their existing flows.
- Timeout retry behavior and native request history identity have focused regression tests.
- The main orchestration loop still owns sequencing and final response selection; this task introduces explicit boundaries around each stateful concern without building a generic orchestration framework.
- No live service, production database, merge, or deployment was used.

## Remaining verification

- Await CI for `cargo test --workspace --all-targets --locked` and PostgreSQL integration workflows.
- Review the resulting diff and CI before considering T15 complete.
