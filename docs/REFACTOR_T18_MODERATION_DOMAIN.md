# T18 — Moderation domain decisions and privilege review

## Scope

Baseline SHA: `356c0cf7e6b4520f903d89300edff373226e4ee9` (T17 verification checkpoint). Branch: `codex/refactor-t18-moderation-domain`; implementation commit `e5de137dd9f0379d029ce97d42ca63cc223f779b`. Draft PR [#151](https://github.com/Mar2ianen/NedoBot/pull/151) targets T17 / PR #150. No merge or deployment has been performed.

## Authorization and existing backend boundaries

- Manual moderation calls `authorize_actor` before target resolution or mutation. The actor must be privileged; restriction commands additionally require both actor `can_restrict_members` and bot `can_restrict_members`. The actor gate runs before `banChatMember`, `restrictChatMember`, `unbanChatMember`, or restriction-undo effects. The test `unauthorized_actors_never_pass_the_manual_effect_gate` pins these cases.
- Manual batch restriction preflights every target and permission snapshot before the first Telegram mutation. Its PostgreSQL reservation uses sorted per-target advisory locks and pending operation rows; each effect must be claimed before it can be sent. Undo claims the recorded prior action under the same target lock and restores that specific recorded action or fails closed on conflict/unknown state.
- Automatic moderation already dispatches through `teloxide_antispam::policy::AutoAction` (`None`, `DeleteMessages`, `Ban`), not through the Telegram command handler. This task preserves that typed backend route. Enforcement still requires managed-chat/config gates, a fresh audit or recent activity for a known spammer, a live eligible Telegram membership, and the current `not_spam` veto. Ban decisions retain their independent-evidence requirement. The repeated `not_spam` checks immediately before captcha/delete/ban effects remain in place.
- Decision score and thresholds are unchanged. Golden cases pin review threshold 70 and ban threshold 90: 69 → none, 70/89 → review deletion, 90 → ban proposal. These fixtures verify existing policy behavior; they do not tune or authorize enforcement.

## Changes

- Captcha solve/expiry and failed-delivery restoration share the per-target advisory lock with manual restriction reservation/undo. Pending, processing, applied, and unknown manual restrictions block permission restoration. The lock is held through the Telegram read/effect and the challenge state transition.
- Successful captcha answers now evaluate a small pure policy from the **live** Telegram member state. A stale challenge marker cannot authorize a restore if the member is already normal, banned, left, or restricted by another source. A member already banned is never unbanned by captcha expiry: captcha itself only creates a restriction and does not own that ban.
- Decision source freshness is rechecked immediately before captcha enforcement and after permission restoration; edited/deleted source, a newer normal verdict, or `not_spam` veto prevents a new ban and marks the challenge overridden.
- Added a PostgreSQL race scenario for a manual mute arriving while captcha restoration holds the target lock, two reviewers claiming a batch, pending/processing restriction blocking a restore, and parallel action/undo serialization.
- Manual authorization and captcha policy tests cover the no-effect gate, live-member stale snapshot, foreign restrictions, and the unchanged scoring thresholds.

## Verification

All commands ran against the T18 source branch. PostgreSQL tests used disposable local databases recreated with `./scripts/dev_db.sh reset-test`; both databases were migrated before tests.

- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `cargo check --lib --locked` — passed before the final pure policy extraction; final full workspace tests and Clippy compile that extraction.
- `cargo test --lib --locked features::risk_captcha::tests` — 6 passed, 0 failed, 628 filtered.
- `cargo test --lib --locked telegram::manual_moderation::tests` — 6 passed, 0 failed, 628 filtered.
- `cargo test --lib --locked features::auto_moderation::tests` — 8 passed, 0 failed, 626 filtered.
- `TEST_DATABASE_URL=postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_test cargo test --features manual-moderation --test manual_moderation --locked -- --ignored` — 4 passed, 0 failed; includes batch preflight, duplicate retry, warning escalation, and the new captcha/manual-action race scenario.
- `RUST_MIN_STACK=33554432 TEST_DATABASE_URL=postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_test cargo test --test postgres_migrations --locked -- --ignored` — 1 passed, 0 failed.
- `CARGO_PROFILE_TEST_DEBUG=0 cargo test --workspace --all-targets --locked` — 706 passed, 0 failed, 14 ignored across 40 targets. Debug symbols were disabled only for this local test-profile command to avoid a multi-gigabyte linker artifact; repository configuration is unchanged.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed on the completed diff.
- The first default-debug workspace rerun was stopped after the linker wrote more than 3 GB without finishing; the compact-profile full rerun above completed successfully.

## API, schema, behavior, and remaining limits

- New private Rust policy surface: `CaptchaMemberState`, `CaptchaSolveDecision`, and `captcha_solve_decision`; new internal repository helper `lock_automatic_restriction_restore`.
- No new dependency, SQL migration, public config key, enforcement threshold, spam score, or sanction policy was added.
- Telegram has no compare-and-set membership API. The code obtains a live membership immediately before the effect and serializes this bot's own manual moderation with database advisory locks. A human administrator using another bot/client can still change membership between Telegram's read and write; that external race cannot be fenced by PostgreSQL. When the live state is already inconsistent, the code fails closed and preserves the other restriction.
- GitHub Actions run [#43905648621](https://github.com/Mar2ianen/NedoBot/actions/runs/43905648621) passed formatting, installer/concurrency checks, workspace tests, and Clippy, then remained in `Apply PostgreSQL migrations` for over 30 minutes. The disposable local migration and integration tests passed. To keep this CI step from producing multi-gigabyte debug artifacts, `.github/workflows/ci.yml` now sets `CARGO_PROFILE_DEV_DEBUG=0` only for the two migration-tool invocations; this changes no runtime or test behavior. Recheck the new head's Actions run before closing T18. No production service, production database, credentials, merge, or deployment has been touched.
