# T16 — URL domain boundaries and future policy readiness

## Scope

Baseline SHA: `c5dfb8e4dc1c2afb5512f05a149620a390adc054` (T15 CI-success docs checkpoint). Branch: `codex/refactor-t16-url-domain-boundaries`; implementation commits `5cd5b829df82e3e9b14d9c3d1061c19855db2e3a` and `08b3347d21f18041a19a43b1c15f5aab75399c9a`; current PR [#149](https://github.com/Mar2ianen/NedoBot/pull/149) head `08b3347d21f18041a19a43b1c15f5aab75399c9a`, stacked on T15 PR #148. No schema, migration, dependency, or public API change is intended.

## Changes

- `telegram::url_targets` is the raw Telegram extraction adapter. It delegates to `teloxide-url`; the evidence store consumes its full typed result, while relation extraction filters that same result to URL targets. It does not implement parsing.
- URL evidence identity/normalization, provenance naming, privacy classification, and SHA-256 encoding now live in `db::url_evidence::identity`. Existing public QR evidence types and caller-facing database functions remain available at their prior paths.
- SQL materialization is grouped behind `UrlEvidenceStore`, while retaining the existing transaction, replacement, fingerprint, and redaction behavior.
- Pure Telegram link relation and QR relation classification are isolated in `features::url_relations::domain`; existing public relation function signatures continue to delegate to the same rules.
- Both new-user audit and refresh paths use `UrlShadowProjection`, which converts the existing relationship observation to the existing advisory `teloxide-antispam` shadow analysis. Malformed counts fail closed to zero.
- Added cross-source canonical identity coverage (visible text, Telegram entity, RichMessage, QR), native-user-ID identity coverage, source-neutral projection coverage, and malformed-projection coverage.
- Added [`URL_IDENTITY_ADR.md`](URL_IDENTITY_ADR.md), defining future ServiceId and ResourceId separately and recording Ozon cross-region links as a possible resolver candidate only after authoritative equivalence is proven.

## Compatibility review

The open alias resolver PR #127 (head `6b24f04bad159fdb02061295a70449cee3c77ff6`) and registrable-domain inventory PR #128 (head `80370ea3c2b5d3bf0fc7835a90f30ed8d5cbd9ac`) were reviewed as separate drafts. Their CI was green at review time. T16 does not import their branches, add the `psl` dependency, or edit inventory CLI/docs. PR #127 also modifies `url_relations.rs`; its alias-aware classifier is inserted alongside the existing classifier, so that overlap must be rechecked after either branch is rebased or merged. Alias freshness and ambiguous-owner fail-closed behavior are covered by #127, not claimed as new T16 behavior.

## Verification

- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `cargo check --lib --locked` — passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `cargo test --lib --locked url` — 54 passed, 0 failed, 4 ignored, 564 filtered out before the final adapter wiring. Includes the new Unicode canonical identity test, QR relation and shadow projection tests, plus existing hidden-link, edit fingerprint, and privacy coverage.
- After final adapter wiring: `cargo fmt --all -- --check`, `git diff --check`, `cargo check --lib --locked`, and the canonical cross-source identity test — passed.
- Full GitHub Actions for the final PR head — pending.
- PostgreSQL evidence/privacy tests on the disposable local `tg_ai_bot_test` database (the local test databases were recreated with `./scripts/dev_db.sh reset-test` and migrated before these tests):
  - `TEST_DATABASE_URL=postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_test cargo test --lib --locked db::url_evidence::tests::database_evidence_is_redacted_replaceable_and_backfillable -- --ignored --exact` — 1 passed.
  - `TEST_DATABASE_URL=postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_test cargo test --lib --locked features::new_user_analysis::tests::url_shadow_observations_are_separate_and_private -- --ignored --exact` — 1 passed.
  - `TEST_DATABASE_URL=postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_test cargo test --lib --locked features::new_user_analysis::tests::qr_only_evidence_refreshes_shadow_and_removed_links_clear_positive_state -- --ignored --exact` — 1 passed.
- Adversarial harness PR #129 was not merged into this base. Its CI and the separate #127 alias resolver CI are not represented as T16 tests.

## Invariants and remaining verification

- No URL allow/deny enforcement, new moderation score, sanction, or network URL dereference was added.
- Raw URL parsing remains owned by the pinned `teloxide-url` dependency. Provenance remains source-specific; canonical target identity is source-independent. Persisted evidence continues to store SHA-256 fingerprints and redacted hints, not raw targets.
- No fake Ozon or YouTube domain allow was introduced. No new URL trust decision is made.
- Public function signatures and persisted schema are unchanged; no migration or dependency was added.
- Full workspace CI on the final PR head remains pending. PR #127/#128 overlap should be revalidated when they are rebased or merged.
- No live service, production database, merge, or deployment was used.
