# T07 — Read-only command adapters

## Scope

Baseline: `7b8815bbff80c4665b64076a063f5f4919e5465a`. This task is stacked on T06 branch `codex/refactor-t06-router`, current head `0121f91`. Work branch: `codex/refactor-t07-readonly-commands`.

## Changes

- Routed `/help`, `/ping`, `/db`, `/memory`, chat stats/status, top/bottom reports, `/topword`, `/userstats`, and `/userstatus` through `telegram::command_handler::read_only` and its stats adapter. Remaining write/action commands retain the existing central command match.
- Added a small typed stats command context carrying `ChatScope` (output and query chat), `Actor` (reply and sender IDs), and `Audience`; period and render remain the existing `StatsPeriod` and `StatsRender` enums.
- Added a parse/dispatch route table covering utility commands, stats periods and aliases, ephemeral/render flags, topword, and the un-routed emoji command.
- Kept authorization, managed-chat checks, stats/reports gates, command ingestion, and manual-moderation filtering in the shared outer adapter before routing. The command menu remains informational and does not grant permissions.
- Kept `StatsReportData` and the existing HTML/Rich renderer pair as the shared output contract. SQL, 05:00 MSK windows, card text, output argument parsing, usernames, reply fallback, and service-message audience selection were not changed.
- Feature-gated the extracted callbacks router module so `--no-default-features` builds stay warning-free.
- No new dependencies or migrations.

## Verification

Local:

- `cargo fmt --all -- --check` — passed.
- `CARGO_TARGET_DIR=../../../../target cargo check --workspace --all-targets --locked` — passed.
- `CARGO_TARGET_DIR=../../../../target cargo check --workspace --all-targets --no-default-features --locked` — passed without warnings after callback module gating.
- `CARGO_TARGET_DIR=../../../../target cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `git diff --check` — passed.

The full test binary was not rerun locally due the already observed long rustc codegen and nearly full home filesystem; all test targets type-check locally. Full tests and PostgreSQL repo checks will run in the stacked PR CI.

## Review state

No merge or deployment. PR and CI links will be recorded after publication.
