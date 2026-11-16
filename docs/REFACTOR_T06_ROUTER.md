# T06 — dptree router decomposition

## Scope

Baseline: `7b8815bbff80c4665b64076a063f5f4919e5465a`; this task is stacked on T05 branch `codex/refactor-t05-ingress-effects`, head `0e00edb`. Work branch: `codex/refactor-t06-router`.

## Changes

- Moved dispatcher graph assembly out of `main` into `telegram::router::build(Routes)` and split route composition into commands, messages, reactions, callbacks, and members modules.
- Kept endpoint implementations and their effects in `main`/domain adapters. The router accepts typed `UpdateHandler<RequestError>` values; root dispatcher dependencies remain `AppState` and `ProfileRefreshQueue`.
- Preserved exact precedence: command filter before ordinary message fallback; then message reaction, reaction-count, edited-message, optional moderation callback, and chat-member branches. Branches remain mutually exclusive update routes; independent application effects continue to be orchestrated explicitly inside the message handler.
- Preserved message endpoint sequence without moving domain work: unmanaged/private gating; required ingest when applicable; mention/reply Ask gateway (including `ingest=false`); then for persisted ordinary messages reply-userstats, voice auto-transcription, and first-comment enqueue.
- Kept callback endpoint's internal order unchanged: risk captcha, report, then spam review. Member and reaction chat scope checks remain in their existing endpoint handlers.
- Moved the production command-first route characterization tests from `main` next to the router helper. No dependency or behavior/config change.

## Verification

Local `cargo fmt --all -- --check`, `git diff --check`, `CARGO_TARGET_DIR=../../target cargo check --workspace --all-targets --locked`, and `CARGO_TARGET_DIR=../../target cargo clippy --workspace --all-targets --locked -- -D warnings` passed. A redundant local `cargo test --no-run` attempt was stopped after prolonged rustc output waits on the nearly full home filesystem; `cargo check --all-targets` type-checked all test targets. Full tests and integration suites will be verified by PR CI.

## Review state

Focused stacked draft PR; no merge or deployment. Final commit, PR number, and CI results are added after publication.
