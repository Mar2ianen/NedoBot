# NedoBot modular refactor execution

Work follows the 22-task pack dated 2026-11-16. One focused PR per task; no merge or production deployment without a separate explicit request. Each task must satisfy its dependencies and release gates before higher-risk work starts.

## Progress

| Task | Status | Branch / PR | Notes |
| --- | --- | --- | --- |
| T00 — baseline snapshot | Implemented; draft PR #133, CI passed 2/2 | `codex/refactor-t00-baseline` | [PR #133](https://github.com/Mar2ianen/NedoBot/pull/133), head `6508013e8d09ed832a50185ae5a4b900a9d595c1`. Report: [REFACTOR_BASELINE_2026-11-16.md](REFACTOR_BASELINE_2026-11-16.md). Baseline SHA `7b8815bbff80c4665b64076a063f5f4919e5465a`; fmt, check, 659 default tests, Clippy, no-default check and 9 additional local PostgreSQL integration cases passed. CI runs `43780665554` and `43780673778` passed. |
| T01 — AGENTS and style guidance | Implemented; draft PR #134, CI passed | `codex/refactor-t01-guidance` | [PR #134](https://github.com/Mar2ianen/NedoBot/pull/134), stacked on #133. Preserved the existing AGENTS content while correcting stale paths, the `dev` lag condition and command-handler instruction; installed normative style/test guidance and build matrix. Script dry-run/idempotence, tracked-path/security-invariant checks and `git diff --check` passed. CI run `43781857310` passed. |
| T02 — test/adversarial harness | Implemented; draft PR #135, CI pending | `codex/refactor-t02-test-harness` | [PR #135](https://github.com/Mar2ianen/NedoBot/pull/135), head `d37bdf3208ab5736a6a0a65ec1f48c16fc200724`, stacked on #134. Report: [REFACTOR_T02_TEST_HARNESS.md](REFACTOR_T02_TEST_HARNESS.md). Adds typed Telegram Update fixtures and tests the production command-first dptree route with fake effect counters, chat/ingest scope and manual-moderation feature gate. Default workspace tests 664 passed/0 failed/13 ignored; Clippy, `scripts/test.sh` (9 DB-backed cases), doc tests and targeted no-default gate passed. Full no-default test suite exposes 19 existing profile-vs-feature fixture failures. Open PR #129's adversarial harness passed 4/4 at its current head and is referenced rather than copied. |
| T03–T21 | Not started | — | Follow dependency graph in the task pack `tasks/INDEX.md`; update this table at each task boundary. |

## Baseline release context

- Base: `main` at `7b8815bbff80c4665b64076a063f5f4919e5465a`.
- Current latest deployment tag reachable from this base: `deploy-2026-11-13-decision-score-captcha` (`a0274a48be792661482e85ac3b721cd7420838bb`). This is a source-control observation, not a live deployment check.
- `origin/dev` is an ancestor of `main` but is still at `61c1a9374993123433a78b842d9387607040fa94` (2026-10-07). T00 targets `main`, per the task pack's base and PR sequence.
- Open PRs #127–#129 were verified separately and are not included in the baseline SHA. Recheck their status and heads before tasks that could overlap their files.

## Safety boundary

Refactor work and its tests use local disposable resources only. Do not touch production VPS/DBs, credentials, live Telegram chats, deployment tags or external services. Do not change spam scoring, sanctions, URL shadow policy, model routing or privacy semantics as part of behavior-preserving refactor tasks. Shorts/TikTok remains an architecture example only; do not implement a downloader or command.
