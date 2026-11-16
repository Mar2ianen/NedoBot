# T22 — Final audit of the T00–T21 refactor

## Scope and safety

This correction pass is based on the T21 checkpoint `cd591188d2fd86fb7f823c553a5783ca0034f4d7` and is developed on a separate branch stacked above it. It addresses remaining concurrency, pool starvation, delivery recovery, test determinism, and release evidence findings. Work uses only the disposable local PostgreSQL databases. No production credentials, database, Telegram API, model provider, merge, or deployment are used.

## Confirmed findings and corrections

- Decision CAPTCHA setup had no shared target fence around its first `restrictChatMember` call. A manual mute/ban could race with that effect. Setup now takes the same per-target advisory transaction lock as manual moderation and holds it through the live membership read, Telegram restriction, and persisted ownership marker. It refuses to apply the restriction when a pending, processing, applied, or uncertain manual restriction owns the target.
- CAPTCHA solve and expiry held a transaction/target lock while checking source freshness through `PgPool`. With a one-connection pool this could wait forever for a second connection. The decision-source repository now accepts the existing transaction connection, and all source checks inside solve/expiry use it. The same path is used by setup.
- The manual moderation race test now waits until PostgreSQL reports the exact advisory lock waiter in `pg_locks`; it no longer treats a 50 ms timeout as evidence of a race.
- Parallel manual moderation integration tests used the same timestamp offset for two target IDs and could collide. Their fixture ranges are now distinct so the full parallel test command is repeatable.
- The DB-backed source-current check runs under `PgPool(max_connections = 1)` with a bounded timeout.
- A fake Telegram server applies the initial restriction but returns an API error before the DB ownership marker. The retry observes matching restricted permissions with no CAPTCHA marker and fails closed as `Overridden`; it does not issue a second restriction or a restore. This verifies safe behavior, but an externally ambiguous Telegram result can leave the user restricted until an administrator reconciles it; Telegram provides no idempotency key or ownership token for `restrictChatMember`.
- Worker supervision tests cover panic, unexpected normal return, and task abort. The first two also verify sibling shutdown and drain.
- Voice delivery has a combined DB/fake Telegram regression: confirmed preview, unknown file-send result, persisted `delivery_unknown`, pool close/reopen, and no replay after restart.

## Verification of earlier review fixes

| Finding | Evidence checked | Result |
| --- | --- | --- |
| T05: disabled ingest must not touch PostgreSQL | `ingest::tests::disabled_ingest_skips_before_any_database_effect` uses an unreachable lazy pool and returns `IngestDisabled` | Verified |
| T09: unsupported callback must be acknowledged without mutation | `unsupported_callback_is_acknowledged_without_side_effects` captures the Bot API call and asserts `AnswerCallbackQuery` and no callback text | Verified |
| T11: critical worker panic, normal return, or abort must fail foreground supervision | Three `WorkerSupervisor` tests cover these exits; panic and normal return also assert sibling shutdown | Verified |
| T17: unknown delivery cannot be replayed; only a typed payload rejection may use HTML fallback | `delivery` certainty tests, voice unknown/partial-send tests, and the final negative fallback cases for generic `Bad Request` and insufficient rights | Verified; generic-message fallback was corrected in this pass |
| T19: Ask/provider failure classification must use typed error sources, not message text | `ask_failure_categories_come_from_typed_error_chain_tags` and `untagged_error_text_does_not_change_failure_category` | Verified |
| T10/T19: unknown persisted job states must fail closed | Status round-trip/unknown-value tests cover voice, audit, materialization, decision, and CAPTCHA states | Verified |
| T02 feature fixtures and gates | Full default and `--no-default-features` workspace test suites on the final implementation tree | Verified |

The worker supervisor tests establish that unexpected exits reach the foreground service error path. This checkout has no production Telegram bot unit files, so automatic systemd restart and its configured timeout remain unverified. The repository's MCP unit has `Restart=on-failure`; it is not evidence for the bot units.

## PostgreSQL test-thread stack

The failing runner is one `#[tokio::test]` that awaits the full migration and lifecycle suite in sequence. A 2 MiB test thread reproducibly overflows on a clean disposable database; the same test passes at 4 MiB and 8 MiB. This points to the large generated async test future and its test-thread stack, rather than PostgreSQL state. CI and `scripts/test.sh` now use 8 MiB, which retains a 2x margin over the smallest passing value and reduces the prior 32 MiB reservation by 75%.

The migration runner remains a large single test. Splitting it would make stack use more local, but also requires carefully isolating migration-reset assumptions and fixtures; this pass keeps the behavior stable and records that as a future test-harness improvement.

## PR coordination: #127–#132

At audit time all six PRs are still open and based directly on `main`; none is included in the T00–T21 stack.

| PR | Current head | Files / relationship | Recommendation |
| --- | --- | --- | --- |
| #127 Telegram alias resolver | `6b24f04` | Adds local alias history and changes URL/profile refresh paths. Overlaps #131's alias churn work and touches boundaries introduced by T16/#149. Both current-head CI checks passed. | Keep separate; reconcile against T16 before merge. Do not merge beside #131 without resolving shared URL-relation behavior. |
| #128 registrable-domain inventory | `80370ea` | Adds export tooling/docs and parser fuzz script; no runtime enforcement. Both current-head CI checks passed. | Independent candidate; verify privacy/label separation before merge. |
| #129 adversarial harness | `f648e15` | Adds test-only harness, corpus guidance, and CI. Separate from runtime behavior but modifies shared CI. Both current-head CI checks passed. | Keep as a test-infrastructure dependency; rebase CI edits on T21/T22 and preserve only synthetic public corpus. |
| #130 private URL history corpus | `d59f7c5` | Adds private-history extraction/export and corpus tooling. Both current-head CI checks passed. | Keep separate; require privacy review of extraction and ensure no raw private data is committed or uploaded. |
| #131 alias churn tests | `91f2d8d` | Tests/exporter and `url_relations.rs`; directly overlaps #127. Both current-head CI checks passed. | Do not merge independently until rebased on whichever alias implementation is selected. |
| #132 malformed entity spans | `debc68c` | Telegram entity adapter validation and focused tests; limited overlap with shared test harness only. Both current-head CI checks passed. | Independent review/merge candidate; it does not depend on URL aliases. |

This pass does not merge, close, or rewrite those PRs. PR state and heads should be refreshed immediately before acting because they can change independently.

## Worker shutdown and systemd evidence

`main` now returns an error when a registered long-lived worker exits unexpectedly, which is necessary for a process supervisor to restart it. The repository contains a `Restart=on-failure` unit for the NedoNews MCP service, but no systemd unit files for the Telegram bot instances. Therefore restart policy, `TimeoutStopSec`, and drain behavior of the production bot units cannot be verified from this checkout; this PR makes no claim about their live configuration.

## Verification

Recorded local verification:

- `cargo fmt --all -- --check`, `git diff --check`, `cargo check --workspace --all-targets --locked`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `cargo deny check` passed.
- `CARGO_PROFILE_TEST_DEBUG=0 cargo test --workspace --all-targets --locked --quiet` passed: 635 NedoBot library tests, 21 agent-runtime tests, 31 provider-access tests, 7 QR tests, and all non-ignored integration/unit targets. The NedoBot library reported 7 ignored DB-backed tests.
- `CARGO_PROFILE_TEST_DEBUG=0 cargo test --no-default-features --workspace --all-targets --locked --quiet` passed: 265 NedoBot library tests, 21 agent-runtime tests, 30 provider-access tests, 7 QR tests, and all non-ignored targets. The library reported 1 ignored DB test.
- `scripts/test.sh` passed on freshly reset disposable local databases. It includes the clean PostgreSQL migration/lifecycle suite, four manual-moderation tests, MCP subprocess/two-database isolation, URL/privacy integration tests, and the new voice partial-delivery/restart test.
- The migration runner passed at 4 MiB and 8 MiB and overflowed at 2 MiB on clean test DBs. CI and `scripts/test.sh` use 8 MiB.
- PRs #127–#132 were checked at the recorded heads above; all remain open, and both GitHub Actions checks pass for each.

The bot service's production systemd configuration and production deployment state remain intentionally unverified. This branch still needs its own exact-head CI after publication.
