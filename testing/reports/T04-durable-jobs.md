# T04 — Durable jobs, leases and cross-worker ordering

**Base:** T03 PR head `b6ad9e7c81a44121543a4a549e774226423da1a8`.

## Invariants and counterexamples

| Invariant | Minimal counterexample and independent oracle | Mutation / defect detected | Layer |
|---|---|---|---|
| A worker claim skips a row currently locked by another transaction and takes the next due row. | Two ready post-comment jobs share a due timestamp; the test transaction holds `FOR UPDATE` on the older ID; `pg_stat_activity` must not show the claimant waiting on a lock, and the claim must return the other ID before release. | Remove `FOR UPDATE SKIP LOCKED`: the test observes PostgreSQL's `wait_event_type = 'Lock'` for the competing claimant. | Real PostgreSQL; explicit transaction lock and server wait-state oracle. |
| A stale fence cannot finalize a reclaimed job. | Worker A claims, its lease expires, worker B reclaims, then A finalizes. The persisted job must remain owned by B; the result is `LeaseLost`. | Remove the claim-attempt predicate from a finalizer update. | Existing disposable-PostgreSQL lifecycle assertions for comments, voice, embeddings, history and new-user audit. |
| Retry work is unavailable before its database `next_attempt_at`; attempt and reclaim counters are monotonic. | A job is placed in `retry_wait` with a future due time; claim returns no job; after making it due, the next claim increments attempts. | Make claim ignore due time or reset/decrement attempts during reclaim. | Existing PostgreSQL voice, comment and review-delivery lifecycle checks. |
| Ambiguous delivery is terminal for automatic workers; restart cannot silently repeat the send. | Start a delivery attempt, record `delivery_unknown`, then claim again. Normal claim returns no job; only the explicit operator-reconcile path can reopen it. | Treat expired `sending` as ordinary `processing`/`retry_wait`, or include `delivery_unknown` in automatic claim selection. | Existing PostgreSQL comment and voice lifecycle plus operator retry audit assertions. |
| Persistent state survives worker restart and expired pre-effect work can be reclaimed. | Processing claim A expires without an external effect; a later worker claims generation B; A's finalizer loses its fence. | Reclaim without a new fence or allow stale completion after reclaim. | Reference trace model and PostgreSQL lifecycle assertions. |

The new reference state machine is independent of the SQL repos and uses generated typed operations: claim, heartbeat, begin external effect, successful/retry/failed finalization, unknown delivery, clock advance, restart, cancel and duplicate ingress. It checks 256 seeded traces with shrinking, monotonic fences/attempts, stale-worker rejection and terminal unknown-delivery behavior. It does not substitute for PostgreSQL: the pre-existing domain SQL lifecycle suite remains the oracle for persisted CAS, due-time and delivery state.

Replay the property counterexample space with:

```sh
sh scripts/replay_quality_case.sh durable-job-state-trace 72623859790382856
```

The PostgreSQL lock regression is also run as an isolated ignored integration test by `scripts/test.sh`, after disposable test database migration.

## Verification

- `cargo fmt --all -- --check` — passed.
- Default feature property trace — 1 test passed, 256 generated traces.
- `--no-default-features` property trace — 1 test passed, 256 generated traces.
- Clippy for `adversarial_harness` and `postgres_migrations`, `-D warnings` — passed.
- `cargo test --all-targets` — passed as part of `scripts/test.sh`.
- Ignored `clean_test_database_applies_migrations_and_preserves_comment_job_lifecycle` — passed against local disposable PostgreSQL (92.41 s); this runs the real SQL state-machine assertions, including the new held-row interleaving.
- Isolated `post_comment_claim_skips_a_locked_oldest_ready_job` — passed against local disposable PostgreSQL.
- Mutation check: removing `SKIP LOCKED` made the isolated database test fail after it observed `wait_event_type = 'Lock'` for the competing claim; the production query was restored. The test releases and cleans its synthetic fixture after observation, including the mutant path.

No production DB, live Telegram, private corpus, external endpoint, policy/model change, merge or deploy was used. Existing intentionally ignored tests requiring production-like configuration remain ignored; local database integration was enabled through `scripts/test.sh` only.
