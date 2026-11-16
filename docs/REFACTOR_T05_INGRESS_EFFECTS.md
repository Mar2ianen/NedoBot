# T05 — Ingress and persistence effects boundary

## Scope

Baseline: `7b8815bbff80c4665b64076a063f5f4919e5465a` (the refactor stack runs on top of T00–T04). Branch: `codex/refactor-t05-ingress-effects`; PR [#138](https://github.com/Mar2ianen/NedoBot/pull/138), implementation head `afa2a2b` (draft, stacked on #137).

This slice makes Telegram message projection pure and visible before persistence, and gives ingress a typed result. It does not change moderation policy, SQL schema, transaction boundaries, or externally configured chat behavior.

## Changes

- Added `TelegramMessageRef`, `MessageRevision`, and `TelegramMessageProjection`. Projection extracts message identity, sender/reply/forward metadata, media flags, text/link facts, and raw Telegram JSON without database effects.
- `save_telegram_message()` and `save_edited_telegram_message()` now return `MessagePersistenceOutcome` (`message`, `revision`, and whether the base message row was newly inserted); SQL effect order and per-effect behavior remain intact.
- `ingest_message()` distinguishes unmanaged chat, disabled ingest, and persisted outcomes through `IngressOutcome`. Required persistence errors still fail ingress. Cheap moderation observation remains best-effort and logged; it cannot suppress a successful persistence result.
- Updated `main` callers to match the typed outcome. No new dependencies or migrations.

## Invariants and limitations

- Duplicate Telegram updates still upsert the base row without double-incrementing chat activity or re-enqueueing message embeddings.
- Edits continue through the existing URL/QR superseding and evidence materialization path; replacing a hidden link or editing to plain text removes stale evidence.
- Membership service-message observation remains after the base message persistence.
- These writes are not made atomic by this refactor. A later side-effect error may still follow an earlier committed write; no transaction guarantee is claimed.
- Existing QR changed-file PostgreSQL coverage remains in `tests/url_evidence.rs`; the added ingress characterization covers duplicate/embedding behavior, hidden-link removal on edit, and membership evidence.

## Verification

Verified locally:

- `./scripts/test.sh` — completed successfully using only disposable local `tg_ai_bot_test` and `tg_ai_bot_test_b`; the PostgreSQL lifecycle test containing the new ingress characterization passed, as did the explicit URL evidence, URL shadow, MCP, and manual moderation integration cases.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `cargo check --workspace --all-targets --no-default-features --locked` — passed.
- `cargo fmt --all -- --check` and `git diff --check` — passed.

The full local `cargo test --workspace --all-targets --locked --quiet` rerun was stopped after rustc spent over ten minutes blocked while writing to the nearly full local filesystem. The clean GitHub workflow runs then completed the entire suite: **667 passed, 0 failed, 13 ignored** in each run. PostgreSQL-specific workflow steps passed, including the new migration lifecycle characterization (**1 passed**), URL evidence/shadow checks, RMCP (**2 passed**), and manual moderation (**3 passed**). CI runs [#43790150939](https://github.com/Mar2ianen/NedoBot/actions/runs/43790150939) and [#43790165813](https://github.com/Mar2ianen/NedoBot/actions/runs/43790165813) both passed. No production resource was accessed.

## Review state

Focused draft PR; no merge or deployment. CI is green on `afa2a2b`. The only verification limitation is that the redundant full local workspace test invocation did not finish; the full suite and DB tests passed in both clean CI environments.
