# T10 — Typed lifecycle statuses and errors

## Scope

Baseline: `bd1c702` (T09 report checkpoint, PR #142). Work branch: `codex/refactor-t10-typed-statuses`, stacked on T09.

## Changes

- Added exhaustive Rust enums and strict text decoding for persisted risk-captcha, voice-job, new-user-audit, audit-materialization, decision-job, and decision-action lifecycle statuses. Unknown database values now fail the repository read instead of being treated as a default state.
- Replaced voice phase string arguments with `VoiceJobPhase`; the existing database vocabulary and transition SQL remain unchanged.
- Mapped the main and materialization status returned by new-user-audit claims, plus decision action status returned to its worker, through the same strict decoders.
- Replaced `AskFailureKind::from_error` message matching with `AskFailureTag` values carried in error source chains. Tagged timeout, invalid-action, and tool failures at their source while preserving existing display/source context; untagged errors keep the generation-error fallback.
- Updated test fixtures and PostgreSQL compatibility assertions to use typed status values.
- No migration, schema, dependency, configuration, model-routing, or policy changes.

## Verification

Local:

- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `cargo check --workspace --all-targets --locked` — passed.
- `cargo check --workspace --all-targets --no-default-features --locked` — passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- The filtered test build was started, then stopped before tests ran because the home filesystem had only 3.6 GB free and was filling during a fresh test-profile dependency build. No test result is claimed for that interrupted local command.

Both implementation-head CI runs passed:

- Workspace test suite: **681 passed, 0 failed, 13 ignored** in each run.
- Both runs also passed formatting, Clippy, migration application, and the configured PostgreSQL integrations: URL evidence/privacy/lock/inventory (5 cases), lifecycle (1), RMCP (2), and manual moderation (3).
- Runs: [#43805618261](https://github.com/Mar2ianen/NedoBot/actions/runs/43805618261) and [#43805643436](https://github.com/Mar2ianen/NedoBot/actions/runs/43805643436).

## Invariants and limitations

- Existing status spellings and SQL CHECK constraints are unchanged; there is no data rewrite or migration.
- Unknown persisted values fail closed at the row/repository boundary.
- Ask failure categories no longer depend on provider error wording. Known errors are tagged where they originate; unknown errors intentionally fall back to `generation_error`.
- Full local test execution remains unverified because the test-profile compile was stopped for disk capacity; CI is the verification source for the full suite and PostgreSQL behavior.
- No production database, live bot, credentials, deployment, or merge was used.

## Review state

PR #143 is open and draft, stacked on T09 PR #142. Implementation-head CI passed 2/2. A documentation checkpoint will run before this slice is recorded complete.
