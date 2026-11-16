# T02 — Telegram characterization and route tests

| Field | Result |
|---|---|
| Baseline | `main` SHA `7b8815bbff80c4665b64076a063f5f4919e5465a`; stacked after T00/#133 and T01/#134 |
| Branch / PR | `codex/refactor-t02-test-harness`; [draft PR #135](https://github.com/Mar2ianen/NedoBot/pull/135), head `d37bdf3208ab5736a6a0a65ec1f48c16fc200724`, stacked on #134 |
| Runtime behavior | Message route expression was extracted unchanged into a typed helper shared by production dispatch and tests. No policy, persistence, model, or moderation behavior changed. |
| New dependencies / schema | None |
| New cases | Typed Telegram updates for hidden `text_link`, edited message, bot sender, absent `from` with `sender_chat`; command-first routing; ordinary-message fallback; registered/unknown chat and ingest gate; compile-time manual moderation command gate. Existing `RichMessage` characterization remains in `telegram/rich_content.rs` tests. |
| Independent adversarial harness | Existing PR #129 remains open at `f648e1548328cb2f62792cb41181a1fac870ee60`; its fixed-seed harness was not duplicated. On that exact head, replay script and `tests/adversarial_harness.rs` passed 4/4, including deterministic deliberate-mutation shrink/replay. PR #129 CI runs `43761001356` and `43761007157` passed. |

## Verification

- `cargo fmt --all -- --check` and `git diff --check` — passed.
- `cargo test --workspace --all-targets --locked` — 664 passed, 0 failed, 13 ignored, 0 filtered.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `cargo test --no-default-features --workspace --all-targets --locked` — exposed 19 existing feature-matrix test failures: fixtures load a production profile that enables `ask`/`auto-comment` while those features are intentionally absent. The no-default feature-gating characterization itself passes when targeted.
- Targeted no-default test `telegram::commands::top_word_tests::manual_moderation_command_is_not_registered_without_its_feature` — 1 passed.
- `./scripts/test.sh` — passed on disposable local PostgreSQL databases; migration lifecycle, URL evidence, private URL shadow, QR refresh, manual moderation (3), MCP startup and two-database isolation checks passed (9 DB-backed cases).
- On PR #129 head: `sh scripts/replay_adversarial_case.sh 72623859790382856`, `cargo test --locked --test adversarial_harness`, and `cargo test --workspace --doc --locked` — passed; harness 4/4, doc tests 0 failures.

## Invariants and remaining limits

- Production still dispatches commands before the ordinary message endpoint; tests use the exact helper installed in the dispatcher and fake effects only.
- No production database, VPS, live Telegram API, credentials, external model API, or private chat content was used.
- PR #129 is an independent open PR, not an ancestor of this branch. Its seeded property/replay coverage is verified and referenced rather than copied; keep that dependency visible during PR review/stacking.
- The full no-default test suite remains red for the pre-existing feature-aware fixture issue above; default-feature tests, Clippy, DB script, and targeted no-default command gate pass.
