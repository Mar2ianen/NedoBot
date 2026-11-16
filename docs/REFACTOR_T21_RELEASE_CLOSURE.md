# T21 — Documentation, CI, and release gates

## Scope

Baseline: T20 checkpoint `e9cb355` / draft PR #153. Branch: `codex/refactor-t21-release-closure`; PR metadata is recorded after publication. This change closes the T00–T21 refactor task pack without merging or deploying any PR. It updates developer/release guidance, CI feature-matrix coverage, and the modular-monolith decision record.

## Changes

- Marked older completion/next-step documents as historical and pointed readers to the live T00–T21 status and current architecture map.
- Documented the default and no-default feature test/check lanes, disposable local PostgreSQL setup, and the existing larger async test-thread stack required by the migration lifecycle test.
- Added release gates for exact-SHA CI, clean/upgrade migration checks, artifact hashes, opt-in isolated staging, two-instance health checks, production install evidence, and migration-aware rollback. None runs as an automatic deployment.
- Added an ADR retaining the supervised modular monolith and listing evidence that would justify revisiting ECS/actors/global event buses.
- Added CI lanes for no-default workspace tests and checks. The local `scripts/test.sh` now applies the same `RUST_MIN_STACK` default as CI so local migration runs do not stack-overflow under the default Rust test-thread stack.
- `cargo deny check` initially failed because no dependency license allowlist was configured. Added only the explicit SPDX expressions present in the current locked dependency graph; advisories, bans, and source checks remain enabled.

## Verification

Local checks on this source checkpoint:

- `cargo fmt --all -- --check` and `git diff --check` — passed.
- `cargo test --workspace --all-targets --locked` — 652 passed, 0 failed, 14 ignored.
- `./scripts/test.sh` — passed, including 10 database-backed lifecycle, URL-evidence, manual-moderation, and MCP-isolation cases.
- `cargo deny check` — all four checks passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `cargo test --no-default-features --workspace --all-targets --locked` — 335 passed, 0 failed, 2 ignored; `cargo check --no-default-features --workspace --all-targets --locked` — passed.
- `cargo check --workspace --all-targets --locked` — passed.

The PR's GitHub CI still needs to run on its published SHA; local checks do not replace that immutable-head gate.

Staging and production evidence are intentionally not claimed: this documentation task has no staging target, does not access production credentials, and the task pack forbids automatic deployment.

## Invariants and limits

- No production profile, credentials, database, Telegram chat, migration, spam policy, moderation setting, or runtime feature was changed.
- CI can prove code and disposable-database behavior, but it cannot prove staging latency, real Telegram delivery, or production service health. Those remain operator-triggered release gates.
- All existing T00–T21 draft PRs remain unmerged; deployment remains unchanged.
