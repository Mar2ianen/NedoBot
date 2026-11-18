# NedoBot full testing package — final audit

Audit snapshot: 2026-11-18. Baseline main SHA: `7b8815bbff80c4665b64076a063f5f4919e5465a`. The exact PR stack heads and CI conclusions are reported in the GitHub PRs; this file does not treat unmerged branches as production or mainline state.

## Test contours and evidence

| Area | Evidence |
|---|---|
| Seeded/adversarial harness | Extended the existing PR #129 harness rather than creating a duplicate. Typed Telegram updates, UTF-16 entities, ordered state traces, independent authorization/job models, deterministic seed/replay and shrinking remain in the fast CI lane. PR #155's distinct stacked source was inspected as a compatibility boundary; its files were not copied wholesale. |
| Telegram routing, ingress, durable jobs and moderation | T02–T06 reports add schema-valid update generation, independent event/state oracles, PostgreSQL CAS/lease/concurrency tests, and local Telegram fakes. Database guarantees use disposable PostgreSQL, not mocks. |
| Voice, first comment, Ask, MCP and audit | T07–T13 reports add media byte limits, fake ASR/LLM/MCP seams, cancellation/authorization checks, scoped SQL tests on two databases, model/score boundaries, memory retrieval, stable stats ties and report escaping. |
| QR/media/URL/subprocess | T14 identifies and fixes bracketed private IPv6 and IPv4-mapped private IP filtering, adds bounded seeded QR inputs, byte-limit and child-process timeout/leak tests. The mutation counterexample `http://[::1]/` was rejected after the fix. |
| Configuration/build matrix and release installer | T15 adds explicit default/no-default library checks and a feature-gate test. Release installer temporary-path and concurrent-install tests remain in CI. Removing the manual-moderation feature guard was a killed mutation under `--no-default-features`. |
| CI exit and test-count integrity | T17 wraps cargo test lanes so command failure cannot be swallowed and a successful zero-test run fails. Three synthetic fake-Cargo cases test zero tests, nonzero exit despite a success-looking summary, and aggregated passing summaries. |

## Executed checks

- Exact command `python3 scripts/run_cargo_test_checked.py cargo test --workspace --all-targets --locked`: **714 passed, 0 failed, 17 ignored**. The checked wrapper printed the positive test count and returned success. The 17 ignored include tests that require disposable DB setup or explicit live-like fixtures; they are not counted as covered by this workspace run.
- `scripts/test.sh` on the T14 implementation branch completed its local disposable PostgreSQL migrations, URL evidence, moderation and two-database MCP lanes successfully. It ran 646 root-package tests in its `cargo test --all-targets` stage and 16 additional passing filtered/integration executions. Later T15–T17 changes only affect CI/scripts/docs; they did not alter SQL or service behavior.
- `sh scripts/test_feature_matrix.sh`: default and no-default library checks plus the compile-feature gate test passed under both builds.
- `python3 tests/release_install.py`: 2 passed in temporary install roots; no `/opt` or production paths touched.
- `python3 tests/test_cargo_test_checked.py`: 3 passed. Mutation of a fake cargo command to exit 23 while printing a success-looking summary remained a failure; zero-test success was rejected.
- T14 seeded replays with seed `12345`: URL/IP filtering (4 tests), QR resource property (128 generated bounded PNGs), provider process timeout cleanup and media writer boundary passed. Earlier reports record seeds for other subsystem traces; CI fast lane seed is `72623859790382856`.
- T16 coverage baseline from default workspace all-targets: **42.25% lines, 47.95% regions, 45.91% functions**. No arbitrary percentage gate was added. Stable-toolchain branch coverage was unavailable.
- T16 targeted `cargo-mutants 27.1.0`: both `is_safe_fetch_url -> true` and `is_safe_fetch_url -> false` mutants were caught by independent URL policy tests. A first all-target mutant build failed at link time from output quota and was classified unviable; the lib-only, one-job rerun killed 2/2. The T15 feature-guard mutant also failed as expected.
- `cargo fmt --all -- --check`, workspace Clippy, no-default library check, and `git diff --check` passed during the T14–T16 implementation checks. Workflow YAML parsed successfully and shell scripts passed `sh -n`.

## CI lanes

- PR CI stays deterministic and runs workspace tests, the existing adversarial harness, PostgreSQL integration tests against CI-only databases, release-install tests, Clippy, and default/no-default config checks. Every cargo-test invocation uses the checked runner.
- `.github/workflows/quality-nightly.yml` runs at Monday 03:17 UTC and supports manual dispatch. It uses three fixed synthetic state-harness seeds and a bounded URL mutation run. An optional manual input emits workspace coverage JSON. It has read-only repository permission, no production secrets, no database service, and no arbitrary user URL fetch.
- Coverage and mutation jobs are intentionally outside the fast PR lane. No automatic deployment job was added.

## Open risks and gaps

1. The whole no-default workspace test suite was already red in the T00 baseline (20 config-dependent failures); this work verifies no-default library compilation and a real fail-closed feature-gate test, but does not claim all no-default tests are green. Fixing every config-dependent default-profile fixture remains open.
2. Search fetching is delegated to external MCP providers. Local syntax/IP-literal filtering cannot verify the remote provider's DNS result or each redirect. DNS rebinding and redirect-to-private-address behavior remain unproven; see T14.
3. `cargo-fuzz` and nightly are unavailable in the local environment. No sanitizer fuzz run or fuzz target is claimed; deterministic proptest properties are the executed alternative.
4. Current `main` still launches long-lived workers detached. The separate refactor PR #144 has a correction at code head `8f46b26477fe117e2f6547280f750029bfefd9e4`: unexpected worker completion is fatal after bounded drain, with eight local lifecycle tests. Both exact-head CI runs passed (44194954792, 44194961644). The PR remains open and stacked; this does not change current `main`. T15 remains unverified until the refactor is integrated and the dependent stack is checked.
5. 17 ignored tests in the workspace run are not all covered by `scripts/test.sh`; a live `/ask` smoke is intentionally excluded because it would require external services. Safe PostgreSQL ignored tests are run in dedicated local/CI lanes.
6. Coverage aggregates include binary glue with no direct unit tests and do not establish assertion quality. Low lines in command handling and moderation are prioritized for further focused cases rather than a global coverage threshold.

## PR audit snapshot

- Existing open work: PRs #127–#155 remain open and unmerged, including the corrective/refactor chain. PR #144's worker-exit correction passed CI at its code head and remains unmerged. PR #129 remains the shared adversarial harness foundation.
- Testing stack: #156–#170 are the T00–T14 stack; PR #171 (T15 config/release matrix, draft) and #172 (T16 coverage/mutation profile, draft) are stacked after #170. T17 CI audit is submitted separately after these checks. No branch in this task was merged or deployed.
- CI status is taken from GitHub for the exact commit SHA, not inferred from local test evidence. Any pending or failed check keeps that PR unverified.

## Safety boundary

No production database, Telegram chat, private chat history, external LLM/ASR, public MCP server or user-supplied URL was used. PostgreSQL fixtures used disposable local/CI databases. Synthetic test values and public-source metadata only.
