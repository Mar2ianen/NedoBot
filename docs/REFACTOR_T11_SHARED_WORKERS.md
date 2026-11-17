# T11 — Shared worker lifecycle and observability

## Scope

Baseline: `41024ab2a2823c69dcb12b3e835964ca59894a77` (T10 verification checkpoint, PR #143). Branch: `codex/refactor-t11-shared-workers`, stacked on `codex/refactor-t10-typed-statuses`.

## Worker inventory

The production supervisor owns all 16 long-lived worker identities: /ask memory expiry, spam reputation sync, linked-chat refresh, URL-feed refresh, QR evidence, Decision audit/actions, profile refresh, spammer-avatar embeddings, new-user audit, captcha lifecycle, spam-review delivery, report delivery, first comments, post history, chat retrieval embeddings, and voice transcription. Feature gates and configuration no-op behavior remain at their existing startup call sites. Profile refresh remains bounded at 256 queued items and its configured worker concurrency.

Request-scoped tasks (ask lease heartbeat, concurrent search, HTTP test servers) and subprocesses remain scoped to their existing request/server owners rather than being treated as background queue workers.

## Changes

- Added a typed `WorkerId`, `WorkerSpec`, poll outcomes, and a single `JoinSet` supervisor. Existing domain claim/finalize functions and SQL predicates remain the owners of durable state.
- Centralized poll, idle, failure-backoff, fixed-delay, and interval timing. Existing configured poll intervals and startup cadence are retained.
- Telegram dispatcher shutdown signals workers, stops profile-queue intake, drains already queued profile refreshes, lets in-flight domain operations finish, and waits up to 120 seconds. If the bound expires, remaining tasks are aborted and counted in a redacted warning; existing leases allow durable jobs to recover on the next process start.
- Unexpected worker completion, panic, and abort are logged with a static worker ID. Poll errors use static categories and do not log provider response bodies or tokens.
- Unexpected worker completion, panic, or abort now fails the foreground service: the supervisor signals shutdown, performs the bounded drain, and returns a typed error so `main` exits non-zero. A normal dispatcher shutdown drains workers and returns success. This closes the previous gap where an unhealthy service could continue after losing a worker.
- Extended the read-only lifecycle report with oldest processing age and expired lease count for first comments, embeddings, post history, and review delivery. Existing due age, persisted safe error kinds, terminal failures, and cumulative reclaim counts remain available. No generic job table or schema change was introduced.
- Added worker timing/shutdown/panic/abort tests, bounded profile-queue close-and-drain coverage, and PostgreSQL observability assertions for an expired review lease.

## Verification

Local commands:

- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `cargo test --locked --bin tg_ai_bot_teloxide features::jobs::worker::tests` — passed (8 worker lifecycle tests, including unexpected return, panic, clean shutdown, and bounded abort).
- `cargo check --workspace --all-targets --locked` — passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- Full local test-profile build was not run because the home filesystem reached 100% use with approximately 1.3 GB available. CI is required to verify the full suite and disposable PostgreSQL assertions; results will be recorded after the implementation head completes both required CI runs.

## Invariants and limitations

- No domain state machine, claim SQL, retry SQL, migration, or dependency changed.
- Graceful stop is bounded. If a task ignores cancellation beyond the 120-second drain period, the supervisor aborts it; an unfinished database job is expected to recover through its existing lease. Telegram sends retain their current delivery certainty semantics.
- Lifecycle SQL reports the four queues already covered by the operational report. It does not claim to be a universal metrics backend for every background feature.
- No production database, VPS, credential, live Telegram chat, deployment, merge, or release tag was used.

## CI evidence

Implementation head `18ab7f91a176b6a6dfd3f8eb7e913acfcecf938b` passed both required CI runs:

- [#43810619027](https://github.com/Mar2ianen/NedoBot/actions/runs/43810619027) — 698 passed, 0 failed, 13 ignored across 49 test targets; all configured URL, lifecycle, RMCP, manual-moderation PostgreSQL integrations passed; format, Clippy, and migration steps passed.
- [#43810624501](https://github.com/Mar2ianen/NedoBot/actions/runs/43810624501) — same green result.

The initial implementation CI exposed two test-only races, both corrected without changing worker runtime behavior: the abort test now awaits Tokio's `JoinSet` cancellation result, and profile queue coverage no longer assumes `select!` picks the shutdown branch while both shutdown and a queued item are ready.

The test-only correction head `a2b995c` also passed both current-head CI runs: [#43813637005](https://github.com/Mar2ianen/NedoBot/actions/runs/43813637005) and [#43813643322](https://github.com/Mar2ianen/NedoBot/actions/runs/43813643322). Each run reports 698 passed, 0 failed, 13 ignored; formatting, Clippy, migrations, and all configured PostgreSQL integrations passed.

## Fatal worker-exit correction

The follow-up correction makes unexpected exits fatal after graceful shutdown is attempted. Local verification on the correction worktree: 8 worker lifecycle tests passed, `cargo check --bin tg_ai_bot_teloxide --locked` passed, and `cargo clippy --workspace --all-targets --locked -- -D warnings` passed. The new PR head still requires its own GitHub CI runs; the earlier CI results above cover the prior head only.

## Review state

PR #144 is open as a draft, stacked on T10. Its prior runtime implementation head was `18ab7f9`; head `a2b995c` added deterministic test coverage. The fatal worker-exit correction is an additional unverified PR head pending CI. It is not merged or deployed.
