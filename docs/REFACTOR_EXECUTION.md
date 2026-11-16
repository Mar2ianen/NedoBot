# NedoBot modular refactor execution

Work follows the 22-task pack dated 2026-11-16. One focused PR per task; no merge or production deployment without a separate explicit request. Each task must satisfy its dependencies and release gates before higher-risk work starts.

## Progress

| Task | Status | Branch / PR | Notes |
| --- | --- | --- | --- |
| T00 — baseline snapshot | Implemented; PR pending | `codex/refactor-t00-baseline` | Report: [REFACTOR_BASELINE_2026-11-16.md](REFACTOR_BASELINE_2026-11-16.md). Baseline SHA `7b8815bbff80c4665b64076a063f5f4919e5465a`; fmt, check, 659 default tests, Clippy, no-default check and 9 additional local PostgreSQL integration cases passed. |
| T01 — AGENTS and style guidance | Not started | — | Starts after T00 PR CI passes. |
| T02 — test/adversarial harness | Not started | — | Depends on T00 and T01. |
| T03–T21 | Not started | — | Follow dependency graph in the task pack `tasks/INDEX.md`; update this table at each task boundary. |

## Baseline release context

- Base: `main` at `7b8815bbff80c4665b64076a063f5f4919e5465a`.
- Current latest deployment tag reachable from this base: `deploy-2026-11-13-decision-score-captcha` (`a0274a48be792661482e85ac3b721cd7420838bb`). This is a source-control observation, not a live deployment check.
- `origin/dev` is an ancestor of `main` but is still at `61c1a9374993123433a78b842d9387607040fa94` (2026-10-07). T00 targets `main`, per the task pack's base and PR sequence.
- Open PRs #127–#129 were verified separately and are not included in the baseline SHA. Recheck their status and heads before tasks that could overlap their files.

## Safety boundary

Refactor work and its tests use local disposable resources only. Do not touch production VPS/DBs, credentials, live Telegram chats, deployment tags or external services. Do not change spam scoring, sanctions, URL shadow policy, model routing or privacy semantics as part of behavior-preserving refactor tasks. Shorts/TikTok remains an architecture example only; do not implement a downloader or command.
