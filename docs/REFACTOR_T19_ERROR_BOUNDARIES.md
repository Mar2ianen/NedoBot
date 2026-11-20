# T19 — Error classification and observability edges

## Scope

Baseline: T18 CI-success checkpoint `72c9d53c68052a9e90ac296722061d57fd649ee7`. Final branch head `7ea9a4d1b0d0e841b96f3ffdf79fbadea979dbd`, branch `codex/refactor-review-close-t19`, draft PR [#152](https://github.com/Mar2ianen/NedoBot/pull/152), stacked on draft PR #151. Both CI runs passed: [#43923825414](https://github.com/Mar2ianen/NedoBot/actions/runs/43923825414) and [#43923835306](https://github.com/Mar2ianen/NedoBot/actions/runs/43923835306). This pass also closes the independently reviewed T05, T09, T11, and T17 correctness findings; no production behavior is deployed.

## Changes

- Removed string matching of `429`, `timeout`, and `json` from post-history error classification. Typed LLM transport statuses and reqwest status/timeout causes now map to stable, bounded `error_kind` strings; memory schema/semantic validation uses a typed local error. Existing `http_429`, `timeout`, and `validation_failed` values remain unchanged. Other HTTP status codes are recorded as `http_<status>` so previously collapsed errors are visible.
- Removed text inspection from avatar job failure classification. Download size, download failure, and embedding failure are represented by explicit internal enums and keep their existing SQL values.
- Replaced the T05 size-of-function-pointer test with an async disabled-ingest test that passes a lazy unreachable PostgreSQL pool. A managed chat with `ingest=false` returns the typed skip result without opening a connection or writing.
- Unsupported moderation callbacks now receive `answerCallbackQuery` with no message and no state mutation; a fake Bot API test checks the actual request.
- A panic, abort, or normal unexpected completion of any registered long-lived worker now drains sibling workers and returns an error from `main`, allowing systemd's existing `Restart=on-failure` policy to restore service instead of silently running without the worker.
- Generic Telegram `Bad Request` is no longer treated as a known content rejection. Only the typed parse-entities and message-too-long errors qualify for the payload fallback path.
- Voice tests inject a confirmed preview followed by an unknown document send and verify the pipeline does not resend either message in the same attempt. Existing PostgreSQL lifecycle coverage verifies persisted `delivery_unknown` cannot be claimed again after a worker restart/query.
- Replaced no-default search tests' calls to deployment `Config::from_env()` with a synthetic feature-aware config. Tests requiring absent runtime modules are compile-feature gated. CI now runs both `cargo test` and `cargo check` with `--no-default-features`.

## Verification

Confirmed locally on the final T19 source checkpoint:

- `cargo fmt --all -- --check` and `git diff --check`.
- Targeted tests for supervisor panic propagation, disabled ingest, unsupported callback ACK, typed Telegram delivery classification, voice partial delivery, and typed memory error mapping.
- `CARGO_PROFILE_TEST_DEBUG=0 cargo test --no-default-features --workspace --all-targets --locked` — 335 passed, 0 failed, 2 ignored across the workspace after initially reproducing the 19 feature-fixture failures.
- `CARGO_PROFILE_DEV_DEBUG=0 cargo check --no-default-features --workspace --all-targets --locked` — passed.
- `cargo test --workspace --all-targets --locked` — 652 passed, 0 failed, 14 ignored (633 library, 19 other target tests).
- `./scripts/test.sh` — passed. Ten database-backed cases passed: lifecycle/migration (1), URL evidence/privacy/shadow removal (3), manual moderation (4), and MCP transport plus two-database isolation (2).
- Targeted T19 regression tests for worker panic propagation, disabled ingest, unsupported callback acknowledgement, typed memory error classification, Telegram send classification, and partial voice delivery — passed.
- `cargo deny check` — advisories, bans, explicit licenses, and sources all passed.
- `cargo fmt --all -- --check`, `git diff --check`, and `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed on the final source checkpoint.
- Current branch CI is required before the draft PR can be treated as verified; no CI results are claimed before push.

## Limits

- All currently registered `WorkerSupervisor` tasks are long-lived service workers. An unexpected exit is therefore process-fatal; durable job leases recover after systemd restarts the process. There is no automatic in-process retry loop for a panicking worker.
- Telegram has no send compare-and-set; voice delivery certainty remains terminal after partial or ambiguous sends to prevent duplicate previews.
- PR #129's deterministic adversarial harness remains a separate open PR and is not copied into this change. Keep it visible as an independent review/merge dependency.
- No dependency, SQL schema, status constraint, public API, policy threshold, moderation setting, production credential, service, or database was changed.
