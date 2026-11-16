# T05 — Ingress and persistence effects boundary

## Scope

Baseline: `7b8815bbff80c4665b64076a063f5f4919e5465a` (the refactor stack runs on top of T00–T04). Branch: `codex/refactor-t05-ingress-effects`; PR and final head are recorded here after publication.

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

Commands and exact counts are updated after the final local test run. PostgreSQL verification used `./scripts/test.sh`, which recreates only local disposable `tg_ai_bot_test` and `tg_ai_bot_test_b` databases. No production resource was accessed.

## Review state

Focused draft PR; no merge or deployment. CI results and any remaining limitations are added here after GitHub checks finish.
