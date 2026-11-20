# T08 — Command action adapters

## Scope

Baseline: `7b8815bbff80c4665b64076a063f5f4919e5465a`. Stacked on T07 PR #140, branch `codex/refactor-t07-readonly-commands`. Work branch: `codex/refactor-t08-command-actions`, implementation commit `0200bd6`. Draft PR: [#141](https://github.com/Mar2ianen/NedoBot/pull/141).

## Changes

- Routed Ask, Forget, chat/user notes, reports, notspam, transcription, and feature-gated manual moderation through `telegram::command_handler::actions`.
- Added typed `ParsedCommand` and `CommandInvocation`. The invocation projects an explicit chat scope, actor identity (human, bot, anonymous chat, missing), and reply target. The existing parsed `Command` enum remains the parser output and non-action commands fall through unchanged.
- `/usernote` consumes a reply target only when it belongs to the same chat and has a positive Telegram message ID. The note use case, access policy, and delivery code remain in their existing owners.
- Manual moderation authorization remains in its backend frontend: it verifies actor privilege and operation-specific restriction rights via Bot API, returning an `AuthorizedActor` only after checks pass. Command-menu visibility is not used for authorization.
- Shared unknown-chat, manual-moderation feature/config, ingestion, reports, and stats gates remain before adapters. No changes to batch preflight, undo, or risk policies.
- No dependencies or migrations. Production manual moderation configuration was not changed.

## Verification

Local commands:

- `cargo fmt --all -- --check` — passed.
- `cargo check --workspace --all-targets --locked` — passed.
- `cargo check --workspace --all-targets --no-default-features --locked` — passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `cargo test --locked --lib telegram::command_handler::actions::tests -- --nocapture` — 2 passed, 0 failed, 596 filtered out.
- `git diff --check` — passed.

The test additions cover human, bot, anonymous-chat and missing actors, plus valid same-chat and malformed/cross-chat reply projection. Both implementation-head CI runs passed: workspace **670 passed, 0 failed, 13 ignored**; PostgreSQL checks included lifecycle (1), RMCP (2), manual moderation (3), and URL evidence/privacy/lock integration tests. Runs: [#43797909102](https://github.com/Mar2ianen/NedoBot/actions/runs/43797909102) and [#43797934074](https://github.com/Mar2ianen/NedoBot/actions/runs/43797934074). A documentation-only checkpoint will run CI again.

## Invariants and limitations

- No command was added. Action routing is separate from read-only routing and automatic moderation.
- Manual moderation still fails closed through the existing authorization backend; an actor projection is not authorization.
- This is an adapter extraction, not a full platform-independent command model. Conversation scope resolution and each command's access policy remain in the existing use cases.
- The pre-existing `/usernote` behavior when no valid replying user is available is retained (it follows the existing chat-note fallback); this task does not redesign note syntax.
- No live bot, production database, credentials, deployment, or merge was used.

## Review state

PR #141 remains draft and open, stacked on #140. The implementation-head CI runs are green. No merge or deployment was performed.
