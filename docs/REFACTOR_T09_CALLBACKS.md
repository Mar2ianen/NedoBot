# T09 — Callback, reaction, and member update adapters

## Scope

Baseline: `1bf1e51` (T08 PR #141). Work branch: `codex/refactor-t09-callbacks`, implementation commit `a0ccb36`. Draft PR: [#142](https://github.com/Mar2ianen/NedoBot/pull/142), stacked on #141.

## Changes

- Moved moderation callback endpoint/application handlers into `telegram::callbacks`; `main` now only connects the callback update filter to that endpoint.
- Added typed callback classification for risk captcha, reports, spam review, and unsupported payloads. The original precedence is retained: captcha → report → spam review. Existing feature parsers remain authoritative for their wire formats.
- Kept actor checks, database transitions, captcha protection against overriding manual restrictions, callback answers, card deletion, and report-card edit behavior with their existing services and logic.
- Moved reaction, reaction-count, and chat-member endpoints into `telegram::updates`. The same `managed_chat_allows(..., ingest)` checks, persistence calls, welcome/farewell handling, and their ordering are preserved.
- Added classifier tests for callback kind/precedence and malformed/unsupported values. Existing captcha actor/chat/expiry tests and manual moderation/report PostgreSQL tests remain unchanged.
- No dependencies, schema, or policy changes.

## Verification

Local:

- `cargo fmt --all -- --check` — passed.
- `cargo check --workspace --all-targets --locked` — passed.
- `cargo check --workspace --all-targets --no-default-features --locked` — passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `cargo test --locked --lib telegram::callbacks::tests -- --nocapture` — 2 passed, 0 failed, 598 filtered out.
- `git diff --check` — passed.

The full local `cargo test --workspace --all-targets --locked` build was attempted but stopped after about nine minutes while Rust compiled the large library test target and two binary targets; the home filesystem reached 100% use with 2.2 GB free. It produced no test result and was stopped to recover build space. Full workspace and PostgreSQL tests are therefore delegated to stacked PR CI. Existing integration coverage exercises wrong captcha actor/chat, expired/stale challenge, duplicate report resolution, and manual moderation policy. No callback BotPort/fake-requester abstraction exists in the current code; introducing that outbound port is deferred to the dedicated Telegram delivery-port task rather than adding a new abstraction here.

## Invariants and limitations

- Risk captcha retains first dispatch precedence and all existing user/chat/source/expiry checks.
- Unsupported callback payloads remain no-ops. Stale known callbacks retain their existing callback answers.
- Reactions are persisted only for managed chats whose ingest flag is enabled. Member snapshots are persisted only under the same ingest condition, while existing welcome/farewell handling remains managed-chat scoped.
- Acknowledgment, DB update, and card update ordering remains the pre-existing implementation; no concurrency or API semantics were intentionally changed.
- No production database, live bot, credentials, deployment, or merge was used.

## Review state

PR #142 is open and draft. CI is running on the implementation/report head. No merge or deployment was performed.
