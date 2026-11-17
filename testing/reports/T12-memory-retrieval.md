# T12 — Memory, search/retrieval, embeddings and external results

## Invariants and counterexamples

| Invariant | Independent oracle / minimal case | Defect caught | Layer |
|---|---|---|---|
| Summary entities are trimmed, Unicode-capped with the shared ellipsis convention, case-insensitively deduplicated in first-seen order, and capped at 12. | 256 seeded vectors of schema-valid JSON entity lists, including empty strings, whitespace, duplicate case variants and arbitrary Unicode. The oracle independently trims, counts Unicode scalar values, appends an ellipsis only when truncated, and applies stable deduplication. | Dropping truncation, deduplication, stable order or item cap changes the parsed entity list. | `memory::service::parse_memory_summary` property test. |
| Untrusted error text cannot become transport status or retry categories. | Generated arbitrary error strings are decorated with `HTTP 429 timeout timed out JSON summary=null`; they must remain generic errors. Separate typed `LlmTransportError::HttpStatus(429)`, typed timeout and `serde_json::Error` cases must retain their classifications. | The old substring classifier misclassified `"model text mentions HTTP 429, timeout, and invalid JSON"` as `http_429`; the regression test failed before the fix and passed after typed classification replaced substring scanning. | Error-classification unit/property tests. |
| Context compaction never reports a target met when the independent budget arithmetic says otherwise. | 256 seeded typed sequences of user-message entries and target budgets. The oracle independently totals dropped entry costs and requires them to cover `used - target`, match `freed_estimate`, and leave at most the target. | Incorrect freed estimates, target boundaries, or arithmetic underflow violate the reference budget. | `agent-runtime` compaction property test. |
| RAG stays inside discussion-chat and source-channel scope and ties have deterministic ranking. | Existing PostgreSQL RAG test uses a local fake embedding HTTP endpoint and real disposable PostgreSQL. T12 adds two same-scope rows with identical vectors and a shared timestamp; the expected order is descending source message ID, and foreign-channel/discussion rows are absent. Re-run with `top_k=1` verifies the tie winner. | Removing either scope predicate admits foreign summaries; changing the secondary order or limit returns a different top result. | `assert_first_comment_context_isolated` inside `assert_chat_search_quality_path`. |
| Conversation history stays within its actual profile/chat/reply scope and forget epochs reject late writes. | Existing PostgreSQL tests use separate profile/chat/reply scopes, sibling commands, post-forget epoch mismatch and bounded context, then verify rows loaded from each scope. | Removing a scope key or epoch compare mixes memory or resurrects a forgotten turn. | Existing `assert_ask_conversation_memory` PostgreSQL lane. |

## Replay

```sh
sh scripts/replay_quality_case.sh memory-summary-normalization 12345
sh scripts/replay_quality_case.sh memory-error-classification 12345
sh scripts/replay_quality_case.sh agent-compaction-budget 12345
```

All three property runners use deterministic seeds, shrinking, synthetic inputs, and print case ID/SHA/replay information on failure. The classifier regression was first run against the old implementation and failed with the minimal stable message containing `HTTP 429`; after the `fix:` commit it passes while typed 429 and timeout errors remain distinguishable.

## Scope and limitations

- Fixed the confirmed retry-classification bug in a separate `fix:` commit. Retry scheduling and production scoring/policy were not changed.
- PostgreSQL access and locks are tested against the disposable local databases. Embeddings use the test’s local fake HTTP server; no external model, user URL, chat, production DSN or private data was accessed.
- Existing summary skip-reason, external-fact provenance, per-chat RAG filters, conversation-memory epoch/TTL and agent-runtime context-pair tests were reused. T12 adds direct generated assertions for summary normalization, typed errors, compaction budgets and equal-score RAG ordering.
- CI fast-lane impact is bounded: each new property has 256 cases. The full longer `scripts/test.sh` stays the manual local DB lane.
- One initial full-library run had a transient failure in the adjacent YouTube subprocess fixture: its expected error text differed, without diagnostic detail. The test now prints the actual `ErrorData`; the isolated case passed, the full library suite then passed three consecutive times, and the final full script passed. No repeated YouTube failure was observed.
- `docs/CODE_STYLE.md` and `docs/TEST_QUALITY.md` are absent in this checkout; root `AGENTS.md` and archive T12 contract were followed.

## Verification

- Seeded replay cases above at seed `12345` — passed, 256 cases each.
- `RUST_TEST_THREADS=8 CARGO_BUILD_JOBS=3 sh scripts/test.sh` — 664 passed, 0 failed, 17 ignored across 46 targets/lanes. Includes local PostgreSQL RAG scope/rank and conversation-memory tests, CAS/lease checks, and both MCP database lanes.
- The root library tests passed three consecutive parallel runs after adding diagnostic output to the transient YouTube fixture; each run was 610 passed, 0 failed, 6 ignored.
- `cargo fmt --all -- --check`, `CARGO_BUILD_JOBS=3 cargo clippy --workspace --all-targets --locked -- -D warnings`, `CARGO_BUILD_JOBS=3 cargo check --no-default-features --lib --locked`, and `git diff --check` — passed.
