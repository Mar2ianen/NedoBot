# T06 — dptree router decomposition

## Scope

Baseline: `7b8815bbff80c4665b64076a063f5f4919e5465a`; this task is stacked on T05 branch `codex/refactor-t05-ingress-effects`, head `0e00edb`. Work branch: `codex/refactor-t06-router`; draft [PR #139](https://github.com/Mar2ianen/NedoBot/pull/139), implementation head `97740ea`.

## Changes

- Moved dispatcher graph assembly out of `main` into `telegram::router::build(Routes)` and split route composition into commands, messages, reactions, callbacks, and members modules.
- Kept endpoint implementations and their effects in `main`/domain adapters. The router accepts typed `UpdateHandler<RequestError>` values; root dispatcher dependencies remain `AppState` and `ProfileRefreshQueue`.
- Preserved exact precedence: command filter before ordinary message fallback; then message reaction, reaction-count, edited-message, optional moderation callback, and chat-member branches. Branches remain mutually exclusive update routes; independent application effects continue to be orchestrated explicitly inside the message handler.
- Preserved message endpoint sequence without moving domain work: unmanaged/private gating; required ingest when applicable; mention/reply Ask gateway (including `ingest=false`); then for persisted ordinary messages reply-userstats, voice auto-transcription, and first-comment enqueue.
- Kept callback endpoint's internal order unchanged: risk captcha, report, then spam review. Member and reaction chat scope checks remain in their existing endpoint handlers.
- Moved the production command-first route characterization tests from `main` next to the router helper. No dependency or behavior/config change.

## Verification

Local `cargo fmt --all -- --check`, `git diff --check`, `CARGO_TARGET_DIR=../../target cargo check --workspace --all-targets --locked`, and `CARGO_TARGET_DIR=../../target cargo clippy --workspace --all-targets --locked -- -D warnings` passed. A redundant local `cargo test --no-run` attempt was stopped after prolonged rustc output waits on the nearly full home filesystem; `cargo check --all-targets` type-checked all test targets. Both PR CI runs passed the full workspace suite (**667 passed, 0 failed, 13 ignored**), including the moved command-first route tests. PostgreSQL lifecycle integration passed (**1 passed**), RMCP (**2 passed**), and manual moderation (**3 passed**). CI: [#43792907888](https://github.com/Mar2ianen/NedoBot/actions/runs/43792907888) and [#43792913117](https://github.com/Mar2ianen/NedoBot/actions/runs/43792913117).

## Review state

Focused stacked draft PR; no merge or deployment. Both CI workflows are green on the implementation head `97740ea` plus report checkpoint `d56d585`. The only local verification limitation is that the redundant full test-binary build did not finish; the full suite passed in both clean CI environments.
