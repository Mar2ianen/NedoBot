# T11 — New-user audit, materialization, review and reputation

## Invariants and counterexamples

| Invariant | Independent oracle / minimal case | Defect a failing test identifies | Layer |
|---|---|---|---|
| Component scores and review levels stay within their documented bands for arbitrary boundary inputs. | A seeded generator supplies six schema-valid integer fields (four components, bounded external component, and threshold), including values below zero and above 100. A separate formula clamps components individually, caps external reputation at 12, saturates the total at 100, then applies the 70/40 bands. | Dropping a component clamp, changing the external cap, allowing totals above 100, or changing the threshold bands disagrees with the independent formula. | `ScoreComponents` property test, 256 generated tuples, shrink enabled. |
| A late generation cannot finalize over a newer profile snapshot or current generation. | Existing disposable-PostgreSQL CAS test creates an audit job, changes the snapshot/version while the generation lease is outstanding, then checks stale finalization is distinct and the current generation remains authoritative. | Omitting the version/fence predicate permits stale scores or review effects to be materialized. | Existing `assert_new_user_audit_generation_cas_requires_live_lease_and_current_version` PostgreSQL lane. |
| A failed materialization reuses the completed assessment instead of calling the model again. | Existing PostgreSQL lifecycle test persists successful generation, forces materialization replay, and verifies the saved assessment is reused. | Re-running assessment generation on materialization retry changes the assessment or generation attempt count. | Existing `assert_successful_audit_replays_for_materialization` and `assert_audit_generation_is_durable_before_materialization`. |
| A stale replay does not create a review request. | Existing test advances the authoritative snapshot after generation but before materialization, then verifies `materialization_status=stale` and that no review row was created. | Ignoring the stale snapshot CAS records a review against obsolete evidence. | Existing `assert_stale_materialization_result_is_distinct` PostgreSQL test. |
| Stored model assessments must match the evidence available in their source snapshot. | Unit cases reject a missing first-message assessment when the job snapshot has text, reject a first-message assessment without source text, and reject avatar observations without avatar metadata. | Trusting raw stored model JSON without checking source evidence accepts unsupported assessment fields. | `parse_stored_assessment` unit tests. |
| Reputation and moderation scope remain isolated and respect the latest authoritative label. | Existing disposable-PostgreSQL label/reputation tests plus decision-shadow and known-spammer-risk support tests exercise cross-chat scope, latest `not_spam` veto, recheck-before-effect, lease fencing and action gating. | Counting foreign evidence, ignoring the latest veto, or applying a stale decision violates those assertions. | Existing label journal, decision-shadow, and known-spammer-risk tests. |

## Replay

```sh
sh scripts/replay_quality_case.sh audit-score-components 12345
```

The property defaults to a fixed seed and accepts `NEDOBOT_PROPTEST_SEED`; failures print a case ID, source SHA and exact replay command. It generates typed score tuples rather than arbitrary bytes. The score boundary property checks the external crate’s public scoring API against an independent formula. No mutation was applied to the external crate during this task; the identified mutations above describe the regressions the assertions are designed to reject.

## Scope and limitations

- T11 adds generated score-boundary coverage and a replay entry point. Existing audit lifecycle/CAS/materialization/review/reputation tests were reused instead of duplicating their PostgreSQL fixture machinery.
- The local PostgreSQL lane is disposable. No production DSN, Telegram chat, external LLM/embedding endpoint, or private user data was used.
- The script reports 17 ignored tests; these remain explicitly ignored/manual or require opt-in environments. The T11-relevant database and two-instance tests ran through `scripts/test.sh`.
- `docs/CODE_STYLE.md` and `docs/TEST_QUALITY.md` are absent in this checkout; root `AGENTS.md` and archive T11 contract were followed.

## Verification

- `sh scripts/replay_quality_case.sh audit-score-components 12345` — passed, 256 generated cases.
- `RUST_TEST_THREADS=8 CARGO_BUILD_JOBS=3 sh scripts/test.sh` — 661 passed, 0 failed, 17 ignored across 46 test-target/lanes; includes local PostgreSQL CAS/materialization, manual moderation DB checks, and two-database MCP isolation.
- `cargo fmt --all -- --check`, `CARGO_BUILD_JOBS=3 cargo clippy --workspace --all-targets --locked -- -D warnings`, `CARGO_BUILD_JOBS=3 cargo check --no-default-features --lib --locked` — passed.
