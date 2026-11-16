# T14 — New-user analysis decomposition

## Scope

Baseline SHA: `15be8c1a51236c0ee3fd1d35835345d5d80fca1b` (T13 verification checkpoint). Branch: `codex/refactor-t14-new-user-analysis-decomposition`; characterization commit `1eebce4`; implementation commit `7fcbc756f684850a51361eac1fc9390fcd91098f`. Draft PR #147 will be stacked on T13 PR #146.

## Changes

- Reduced `new_user_analysis.rs` to the materialize/refresh orchestration and its existing characterization/integration tests.
- `audit_snapshot.rs` now owns `SnapshotLoader` and a typed `LoadedAnalysisSnapshot` carrying `NewUserFeatures` plus `MembershipContext`.
- `audit_fingerprint.rs` owns evidence loading, trigger/material revision projections, bounded/redacted audit text, and `FingerprintCalculator` entry points. Snapshot schema and hash material are unchanged.
- `audit_planner.rs` owns runtime analysis config projection and the pure `AuditPlanner`; it produces a typed `AuditPlan` for job enqueue and same-fingerprint in-flight decisions.
- `audit_materializer.rs` owns the existing dynamic column allowlist, transaction writer, and profile-photo DC metadata parser.
- No policy thresholds, fingerprint versions, schemas, query predicates, SQL lock order, or LLM call paths changed. No migration or dependency was added.

## Verification

- `cargo fmt --all -- --check` and `git diff --check` — passed.
- `CARGO_TARGET_DIR=<shared T11 target> cargo check --workspace --all-targets --locked` — passed.
- `CARGO_TARGET_DIR=<shared T11 target> cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `CARGO_TARGET_DIR=<shared T11 target> cargo test --lib features::new_user_analysis::tests:: --locked` — 13 passed, 0 failed, 4 ignored (the four require PostgreSQL).
- Added and ran `canonical_analysis_matches_golden_score_and_snapshot_revisions`: the synthetic fixture remains score `18`, snapshot SHA-256 `eb7a70a8a9dfa6af5d1d466004266634e983f0b8fed56565e79730d71c7d587a`, material-revision SHA-256 `b7633f743c32c860735a1304a5ce75d621eb660114a238c1597483404576b761`.
- `cargo test --workspace --all-targets --locked` — passed; main library 612 passed/0 failed/6 ignored, and all other executed crate/test targets passed.
- Disposable PostgreSQL cases after the refactor: same-user serialization/different-user concurrency (1/1), cancellation releases the session lock (1/1), shadow privacy (1/1), and QR source removal clears positive shadow state (1/1).

## Invariants and limitations

- `with_url_shadow_advisory_lock` remains unchanged: bounded semaphore, session lock, `close_on_drop`, same-user serialization and different-user concurrency are preserved.
- The transaction order remains job → audit → review; the latest `not_spam` veto and stale materialization behavior are untouched.
- Score and canonical snapshot/revision hashes are pinned by an independent synthetic golden fixture. Existing PostgreSQL concurrency and cancellation tests were rerun.
- No external service, live chat, production DB, merge, or deployment was used. PR CI is pending.

## API changes

- New private use-case modules: `audit_snapshot`, `audit_fingerprint`, `audit_planner`, and `audit_materializer`.
- Existing public `new_user_analysis` entry points remain unchanged. Internal snapshot loading now returns a typed DTO instead of a tuple.
- No new dependencies or migrations.

