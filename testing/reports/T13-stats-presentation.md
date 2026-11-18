# T13 — Stats, reports and presentation boundaries

## Invariants and counterexamples

| Invariant | Independent oracle / minimal case | Defect caught | Layer |
|---|---|---|---|
| Top and bottom user rankings have stable order for equal activity. | A synthetic PostgreSQL ledger inserts users in order `30003, 30001, 30002` with counts `2, 2, 1`. An independent in-memory oracle sorts by count in the requested direction and then `user_id ASC`; top list, bottom list and top-user-ID limit are compared with it. | Reversing the tie-break to `user_id DESC` failed the DB test: actual `[(30003,2),(30001,2),…]`, expected `[(30001,2),(30003,2),…]`. | Real disposable PostgreSQL, `assert_stats_rank_ties_follow_the_reference_order`. |
| Every top-N result has a deterministic final key. | Stable SQL keys were added for period top users, top/bottom activity, bot-comment engagement, top reacted messages and user-ID projections. The DB fixture verifies user ranking and limiting; existing stats tests cover windows, counts and renderers. | Removing or reversing the final key can reorder equal-score rows or change which tied row is selected by `LIMIT`. | Stats repository SQL and PostgreSQL integration test. |
| Report fallback treats names/reasons/message text as untrusted and remains bounded. | 256 seeded schema-valid text values are inserted into report reason and message fields, wrapped in synthetic `<script>…</script>` markup and rendered. Require escaped text is present, raw injected markup absent, and output within Telegram’s text limit. | Replacing the typed HTML builder with raw interpolation exposes `<script>` or unescaped metacharacters. | Report fallback property test. |
| Existing statistical window and presentation behavior remains coherent across formats. | Existing disposable-PostgreSQL golden assertions compare shared HTML/Rich stats inputs, fixed windows, Moscow day boundaries, null/incomplete cohorts, totals, and representative top-word values. | Query drift between render formats, date-boundary errors, or null becoming zero breaks existing independent fixture assertions. | `assert_stats_renderers_share_period_data` plus HTML/Rich unit tests. |

## Replay

```sh
sh scripts/replay_quality_case.sh report-render-html-escape 12345
RUST_TEST_THREADS=8 CARGO_BUILD_JOBS=3 sh scripts/test.sh
```

The HTML property uses deterministic seed `12345`, 256 cases and shrinking. The tie-order mutation was executed and killed by the PostgreSQL oracle. The original repository had no explicit final key for several rank queries; the fix adds stable secondary IDs without changing the ranking scores.

## Scope and limitations

- Added stable sort keys to stats ranking SQL in a separate `fix:` commit. Counts, ranking weights, filters and sanction policy were unchanged.
- The equal-score PostgreSQL fixture is deliberately small and synthetic; broader temporal/count aggregation coverage remains in the existing date-window tests. No production database, Telegram service or private data was used.
- The first fixture draft reused a message ID across two synthetic users and failed with a PostgreSQL unique-constraint error. IDs were corrected to disjoint ranges; the final complete disposable-DB lane passed.
- Seventeen tests remain marked ignored/manual in the repository’s full script; none are the T13 stats, report-rendering or ranking cases.
- `docs/CODE_STYLE.md` and `docs/TEST_QUALITY.md` are absent in this checkout; root `AGENTS.md` and archive T13 contract were followed.

## Verification

- `sh scripts/replay_quality_case.sh report-render-html-escape 12345` — passed, 256 generated cases.
- `RUST_TEST_THREADS=8 CARGO_BUILD_JOBS=3 sh scripts/test.sh` — 665 passed, 0 failed, 17 ignored across 46 targets/lanes. Includes stats SQL window/rank fixtures, PostgreSQL migrations, read-only ACLs, and two-database MCP isolation.
- Mutation `user_id ASC` → `user_id DESC` — killed by PostgreSQL ranking fixture with the expected mismatched tie order.
- `cargo fmt --all -- --check`, `CARGO_BUILD_JOBS=3 cargo clippy --workspace --all-targets --locked -- -D warnings`, `CARGO_BUILD_JOBS=3 cargo check --no-default-features --lib --locked`, and `git diff --check` — passed.
