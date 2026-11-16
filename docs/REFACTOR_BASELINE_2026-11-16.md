# NedoBot refactor baseline — 2026-11-16

## Snapshot

- Repository: `Mar2ianen/NedoBot`
- Baseline branch: `main`
- Baseline SHA: `7b8815bbff80c4665b64076a063f5f4919e5465a`
- Baseline date: 2026-11-15 (commit subject: merge PR #126, teloxide URL integration)
- Baseline PR: [#133](https://github.com/Mar2ianen/NedoBot/pull/133) (`codex/refactor-t00-baseline` → `main`, draft).
- Runtime/application code, migrations, configuration, credentials, dependencies and generated artifacts are unchanged by T00.

## Repository and dependency snapshot

The three open work-in-progress PRs inspected against this baseline are #127 (Telegram alias resolver, head `6b24f04bad159fdb02061295a70449cee3c77ff6`), #128 (registrable-domain metrics, head `80370ea3c2b5d3bf0fc7835a90f30ed8d5cbd9ac`) and #129 (deterministic adversarial harness, head `f648e1548328cb2f62792cb41181a1fac870ee60`). Their changes are not folded into this baseline.

Pinned Telegram-related libraries from `Cargo.toml`/`Cargo.lock`:

| Crate | Revision |
| --- | --- |
| `teloxide` fork | `f05b90be40e9838e009281182abfcc3db2319c41` |
| `teloxide-antispam` / `teloxide-url` | `d374a191ae6b6bcd66fbcf8a6646d73138064038` |
| `teloxide-statistics` | `7e2a5859849044121b9e693bc1ed36e7440db139` |

## Feature matrix and runtime instances

The default release build enables `auto-comment`, `ask`, `voice`, `public-mcp`, `moderation`, `spam-sync` and `manual-moderation`. All feature declarations are empty marker features; config validation controls which runtime paths are enabled. No-default-features builds are supported by the baseline (`cargo check --workspace --all-targets --no-default-features --locked` passes). `chat_db_mcp` and `nedonews_mcp_http` require `public-mcp`; `voice_smoke` requires `voice`.

Both production communities use the same default-feature binary with separate runtime profiles/configuration and PostgreSQL databases: NedoNews and PVO. The startup/config path is responsible for selecting each community scope and its enabled capabilities. T00 performs no production connection or mutation.

Relevant scoped/security invariants recorded by the source and config include: MCP read scope comes from validated instance configuration and reviewed DB views; private internal MCP access is distinct from explicitly enabled public exposure; moderation and message deletion are gated per instance/chat; manual moderation overrides are retained; URL evidence and spam decisions have independent shadow/decision gates; new-user audit and captcha paths are opt-in/config-gated; ask memory is scoped by instance/profile/chat/user and reply lineage; Telegram delivery is not blindly retried when outcome is unknown.

## Update and side-effect map

| Telegram input / lifecycle | Main route | Observed side effects |
| --- | --- | --- |
| New message | Update dispatcher → command or ordinary-message handler | Commands may send replies or perform their documented DB/moderation actions. Ordinary messages pass managed-chat/private Ask gates, update membership lifecycle, persist the message and URL evidence, may enqueue QR/embedding/profile work, observe spam signals, run the Ask gateway, and may trigger user statistics, voice transcription or first-comment handling. The Ask gateway is evaluated independently of message-ingest success. |
| Edited message | Edited-message handler → message upsert/edit path | Updates stored message state and derived URL evidence, supersedes stale QR evidence and refreshes link-derived state; downstream audit/embedding refresh is subject to its gates. |
| Message reaction / reaction-count update | Dedicated reaction handlers | Persists the corresponding reaction or count state for stored messages. |
| Callback query | Callback dispatcher | Routes captcha responses, spam-review actions and report callbacks; may change moderation/audit state and send or edit Telegram messages. |
| Chat-member update | Membership handler | Persists member history and handles join/leave greetings or cleanup according to community config. |
| Periodic/background work | Startup-spawned workers gated by feature/config | Includes linked-chat refresh, URL feed, Ask-memory TTL, reputation sync, QR processing, decision audit, profile/avatar refresh, new-user audit, captcha and moderation/report delivery, first-comment/post-history, chat embeddings and voice transcription. Workers can read/write their scoped DB state and some can deliver Telegram messages. |

Order-sensitive ordinary-message flow observed in `src/main.rs`: managed-chat/private Ask eligibility → membership lifecycle handling → ingest (message/evidence plus best-effort decision observation) → profile refresh enqueue → Ask gateway → if ingested, reply-userstats → voice auto-transcription → first-comment processing. This snapshot documents source behavior, not a proof of live Telegram delivery semantics.

## Baseline verification

Commands were run on the clean baseline SHA above. `CARGO_BUILD_JOBS=2` and one shared local Cargo target directory were used after unrestricted parallel linking caused severe local disk I/O contention; this does not affect build outputs or test selection.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo check --workspace --all-targets --locked` | Passed |
| `cargo test --workspace --all-targets --locked` | Passed: 659 tests, 0 failed, 13 ignored, 0 filtered in the unfiltered run |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |
| `cargo check --workspace --all-targets --no-default-features --locked` | Passed (additional feature-gating baseline) |
| `./scripts/test.sh` | Passed against the disposable local PostgreSQL container on `127.0.0.1:5433`; the script recreated only `tg_ai_bot_test` and `tg_ai_bot_test_b`, then applied migrations to both |

The PostgreSQL script ran the default suite (same 659 passed / 13 intentionally ignored / 0 failed) and 9 additional DB-backed cases: migration lifecycle (1), URL evidence (1), URL shadow privacy (1), QR-only evidence cleanup (1), manual moderation (3), and RMCP process/two-database isolation (2). All 9 passed. The specifically filtered invocations reported 587 filtered lib tests for each selected URL DB case and one filtered sibling integration test in each selected RMCP invocation. The 13 ignored tests in the generic suite require disposable PostgreSQL, two-database MCP setup, or live/external configuration; the script ran the applicable local DB cases, but did not run live Telegram, production-like LLM, or external provider smoke tests.

## Dependency / API / regression delta

- Public API changes: none.
- New dependencies: none.
- Database schema or migration changes: none.
- Runtime behavior regressions introduced by T00: none (documentation-only).
- CI workflow already runs format, default test suite and Clippy; its PostgreSQL integration steps include local URL evidence/shadow, migration, MCP two-DB and manual-moderation scenarios. CI has no explicit no-default-features job.

## Known documentation drift and unknowns

- `origin/dev` is far behind `main` (`61c1a9374993123433a78b842d9387607040fa94`, 2026-10-07), with `main` as its descendant. Repo guidance that asks post-deploy work to go to `dev` conflicts with this task pack's main-based PR/dependency sequence; T00 therefore targets `main` and does not advance `dev`.
- `docs/DEPLOYMENT.md` identifies `deploy-2026-11-13-author-text-audit` as the latest actual deployment, while the latest deployment tag reachable from baseline `main` is `deploy-2026-11-13-decision-score-captcha` at `a0274a48be792661482e85ac3b721cd7420838bb` (main is 11 commits ahead). This is documentation drift only; no live deployment state was inspected or changed.
- The exact production config/secret values, database contents, current unit health, Telegram permissions/updates and provider availability were intentionally not queried. Runtime behavior requiring Telegram, external models, or production services remains unverified.
- Baseline tests prove current local behavior only. They do not prove production instance parity, privacy against a compromised DB role beyond existing targeted integration coverage, or behavior under real Telegram API failure/rate limiting.

## Explicit non-actions

No production VPS, production PostgreSQL, credentials, live Telegram chat, remote service or production deployment was touched. No branch was merged and no PR was merged automatically.
