# T17 — Typed outbound Telegram delivery port

## Scope

Baseline SHA: `ea93b62cad3327e18323273fe21b92bf750133fc` (T16 documentation checkpoint). Branch: `codex/refactor-t17-telegram-delivery`, commit `302182191e9918714a9de947eac76855c31fcd40`, draft PR [#150](https://github.com/Mar2ianen/NedoBot/pull/150), stacked on T16 / PR #149. No new dependencies, migrations, schema changes, or teloxide fork changes.

## Changes

- Added `telegram::delivery::{TelegramSendPort, DeliveryEffect, DeliveryFailure, DeliveryErrorKind, DeliveryCertainty}`. Delivery results distinguish `NotAttempted`, confirmed Telegram `Rejected`, ambiguous `Unknown`, and `Confirmed`; error values carry only safe typed categories and an optional RetryAfter duration.
- Added the production Bot adapter for validated HTML replies, RichMessage replies, and document replies. Existing renderers keep ownership of normalization, escaped fallback, request bounds, reply construction, and native RichMessage entities/custom emoji.
- Routed durable first-comment sends through the port. A confirmed message keeps the existing ingest/finalize ordering. An ambiguous result transitions to `delivery_unknown`; confirmed RetryAfter and payload rejection retain the existing retry policy.
- Routed voice rich/text/file delivery through the same narrow port while keeping its lifecycle and fallback rules feature-owned. Confirmed payload rejection permits one bounded text fallback; ambiguous rich delivery never starts another send. If a full transcript's file send fails after its preview was delivered, automatic whole-job resend remains stopped.
- Removed request/error-body logging from the changed fallback and voice delivery paths. Malformed response parsing is now classified as `Unknown`; the spam-review retry fixture was updated to send a valid Telegram API 500 response so it continues testing a confirmed retryable rejection rather than malformed transport output.
- Added fake-port fault injection, custom-emoji renderer coverage, bounded/malformed output validation, and a PostgreSQL first-comment fence case: one acknowledged fake send followed by forced DB finalization rollback remains `delivery_unknown` after lease expiry and cannot be resent.

## Verification

All commands ran locally against branch source; PostgreSQL tests used disposable local `tg_ai_bot_test` / `tg_ai_bot_test_b`, recreated with `./scripts/dev_db.sh reset-test` and migrated.

- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `cargo check --lib --locked` — passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `cargo test --workspace --all-targets --locked` — 703 passed, 0 failed, 13 ignored; all test targets passed.
- `TEST_DATABASE_URL=postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_test RUST_MIN_STACK=33554432 cargo test --test postgres_migrations -- --ignored` — 1 passed, 0 failed. This exercises the first-comment acknowledged-send/DB-rollback fence and the surrounding migration lifecycle.
- Focused counts during development: delivery-port tests 3/3; voice pipeline tests 9/9; first-comment delivery mapping 1/1; first-comment renderer tests 13/13.

GitHub Actions has not yet run for this branch; attach its result after opening the draft PR.

## Invariants and remaining verification

- Delivery uncertainty is not automatically retried as a new Telegram message. Only confirmed rejection follows the existing retry/fallback policy.
- Feature lifecycle, persistence, retry limits, and message composition remain outside the port. The port does not expose raw `RequestError` text to durable callers.
- User/model output continues through existing bounded safe renderers; RichMessage entity and configured custom-emoji behavior is preserved.
- No production service, production database, credentials, merge, or deployment was touched.
