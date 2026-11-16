# T13 — QR scan and shadow refresh decoupling

## Scope

Baseline SHA: `aa7addd10ef8bb9754e10fda39d6b4f29f2f7c80` (T12 verification checkpoint). Branch: `codex/refactor-t13-qr-shadow-decoupling`; implementation commit: `837f71cb5d23186478a4f24ffd349d47db2bfc81`; draft PR: [#146](https://github.com/Mar2ianen/NedoBot/pull/146), stacked on T12 PR #145.

## Changes

- QR evidence materialization now locks and validates the current message/media and QR lease, then commits the evidence, a deduplicated URL-shadow refresh outbox request, and `succeeded` QR status in one transaction. A downstream refresh error can no longer put an already materialized QR scan back into the decode retry path.
- Added `telegram_url_shadow_refresh_jobs`, unique on `(instance_id, chat_id, telegram_user_id, evidence_revision)`. Claims are instance-filtered and use `SKIP LOCKED`; retries are bounded with backoff, and completion is fenced by lease generation. The row contains only the evidence fingerprint, never a URL target.
- Added a separate supervised `QrShadowRefresh` worker. It recomputes the existing observational shadow from current evidence and current URL-feed state, so removed sources clear prior positive observations through the existing computation.
- QR decode uses a process-wide one-slot semaphore held inside the `spawn_blocking` closure. When the caller times out or is cancelled, the detached CPU task retains the permit until it exits, preventing queued cancellations from accumulating unbounded decoder work.
- No dependency, QR parser, OCR, video support, shadow scoring, or public MCP exposure was added.

## Verification

Local commands:

- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `CARGO_TARGET_DIR=<shared T11 target> cargo check --workspace --all-targets --locked` — passed.
- `CARGO_TARGET_DIR=<shared T11 target> cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `CARGO_TARGET_DIR=<shared T11 target> cargo test --lib features::qr_url::tests::timed_out_decode_keeps_the_single_cpu_slot_until_blocking_work_finishes --locked -- --exact` — 1 passed, 0 failed, 616 filtered.

The existing disposable-PostgreSQL URL-evidence integration test now also checks outbox deduplication, redacted fingerprint-only payload, retry state after a synthetic refresh failure, successful QR completion, and rejection of edited media. Existing PostgreSQL tests cover cancellation while holding the URL-shadow advisory lock and clearing positive shadow state after source removal. CI run [#43822719597](https://github.com/Mar2ianen/NedoBot/actions/runs/43822719597) is in progress; its result will be recorded here after completion.

## Invariants and limitations

- QR success means the DB transaction containing evidence and follow-up intent committed. Shadow refresh is independently retried and never re-decodes the image.
- Stale message media invalidates the QR lease and cannot materialize evidence or enqueue a refresh.
- Log fields for refresh jobs are numeric job/attempt identifiers and static error categories; raw URL targets are not stored in the outbox or emitted in logs.
- `spawn_blocking` itself cannot be forcibly stopped. The one-slot permit bounds active detached decoder work; shutdown may wait for the currently running blocking decoder to return.
- PostgreSQL fault-injection behavior and migration application are delegated to the configured PR CI run; no production DB, live Telegram API, merge, or deployment was used.

## API and schema changes

- New DB APIs: `claim_next_url_shadow_refresh_job`, `finish_url_shadow_refresh_job`; `replace_qr_evidence_for_job` now receives the configured instance ID and commits scan completion/outbox atomically.
- New migration: `20261116110000_telegram_url_shadow_refresh_jobs.sql`.
- New worker identity: `QrShadowRefresh`.

