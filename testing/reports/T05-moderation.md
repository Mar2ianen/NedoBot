# T05 — Moderation, CAPTCHA, labels and privilege races

**Base:** T04 PR head `35e4ce085f471e6c31e73ec19277540aadda8375`.

## Invariants and counterexamples

| Invariant | Minimal counterexample and independent oracle | Mutation / defect detected | Layer |
|---|---|---|---|
| Mutating moderation requires a privileged actor and a bot with `can_restrict_members`; read-only moderation queries need only a privileged actor. | Table over owner, admin with/without restriction rights, member, restricted, left and banned roles. | Ignore bot rights: owner actor + admin bot without restriction rights is incorrectly accepted. | Production pure policy helper, exhaustive role table. |
| Bots and privileged members cannot be sanction targets. | Cross every Telegram status with `is_bot = false/true`; expected target eligibility is independently enumerated. | Replace conjunction with disjunction or omit either exclusion. | Production pure target predicate. |
| `not_spam` and stale source suppress automatic actions; boundaries stay at 70/90. | `not_spam → score 100`, edited source → score 100, plus scores 69/70/89/90. | Skip the veto/source check or shift either score comparison by one. | Reference trace model and score-table unit test. |
| CAPTCHA can release only its own restriction and only for its owner. | Foreign callback leaves challenge and CAPTCHA restriction pending; owner callback passes it. A manual mute remains after CAPTCHA completion. | Accept a foreign solver or clear a separate manual restriction. | Reference trace model; existing PostgreSQL/fake-Telegram Decision/CAPTCHA suite. |
| A lost Telegram acknowledgement is uncertain and must not be treated as a rejection. | Local fake reads the complete `banChatMember` request, drops the response; a Telegram 403 response is the definitive-reject control. | Classify network loss as rejection, allowing an unsafe retry. | Local HTTP/TCP fake Bot API. |
| Wrong CAPTCHA answers update attempts atomically and closed challenges reject late callbacks. | Three simultaneous callbacks reach 2 remaining, 1 remaining, and failed; fourth callback is stale. Run through a one-connection PostgreSQL pool. | Remove active-status CAS predicate: the late callback returns `Failed` instead of `Stale`. | Disposable PostgreSQL. |
| Sanctions for one target serialize across workers. | Hold target advisory lock in one transaction; competing `prepare_action` must visibly wait in `pg_stat_activity`; then race mute and ban and allow only one processing action. | Remove target advisory lock or processing-action check. | Disposable PostgreSQL advisory-lock oracle and transaction race. |

## Generated replay

The independent `ModerationModel` generates 256 seeded traces of length 0–64. Operations cover label changes, stale-source edits, automatic scores, actor/bot/target roles, CAPTCHA issue/restrict/solve/expiry, manual undo, uncertain Telegram delivery, moderator disconnect and duplicate events. `proptest` shrinks counterexamples and failure output includes the case ID, seed and minimized synthetic trace.

```sh
sh scripts/replay_quality_case.sh moderation-state-trace 72623859790382856
```

## Verification performed

- `cargo fmt --all -- --check` and `git diff --check` — passed.
- Targeted default-feature tests: authorization table, target privilege table, fake Telegram uncertain outcome, score boundaries and moderation reference trace — passed.
- Targeted no-default-features moderation trace — passed.
- Replay command with seed `72623859790382856` — passed, 256 traces.
- Disposable PostgreSQL CAPTCHA CAS test with `max_connections=1` — passed.
- Disposable PostgreSQL moderation advisory-lock/race test (`max_connections=3`, plus a separate one-connection transactional path) — passed.
- Clippy for the library, adversarial harness, PostgreSQL tests and manual-moderation integration target with `-D warnings` — passed.
- Mutation: bypassing the bot restriction-right check failed on the explicit owner/admin-without-rights row.
- Mutation: deleting the CAPTCHA active-status predicate failed because a late answer returned `Failed`, not `Stale`.
- Mutation: replacing the target advisory lock with `select 1` failed because PostgreSQL showed no competing transaction waiting on the lock. The fixture was cleaned before the assertion. The production lock was restored and the focused race test passed again.
- `CARGO_BUILD_JOBS=3 sh scripts/test.sh` — passed all configured local test lanes, including disposable PostgreSQL lifecycle, CAPTCHA CAS, moderation races, MCP isolation, and URL/QR database lanes.
- `CARGO_BUILD_JOBS=3 cargo test --workspace --all-targets --locked -- --quiet` — 690 passed, 16 ignored, 0 failed.
- Both production mutations were reverted; final disposable PostgreSQL and authorization tests passed after restoration.

No live Telegram, production database, external model, policy threshold change, merge or deploy was used. The generated state model is an independent reference oracle; real SQL CAS and advisory-lock behavior are checked separately against disposable PostgreSQL.
