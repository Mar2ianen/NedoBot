# T03 — Ingest, edits, URL provenance and QR generations

**Base:** T02 branch `codex/testing-t02-updates`, head `81bf0f3d914881f51efc6dc5359f9d2877406c8c`.

## Invariants, traces and defect targets

| Invariant | Minimal synthetic counterexample / independent oracle | Defect detected |
|---|---|---|
| A duplicate delivery does not duplicate or change current derived evidence. | Hidden URL A, edit to hidden URL B, then replay B. SQL rows and target hash are queried after each transition. A test-side SHA-256 reference uses the documented `nedobot-url-evidence-v1\0url\0` input format, not production `encode()`. | Missing conflict handling, accumulating stale rows, or replay changing the current target. |
| Removing a link removes its old evidence; the latest state represents the edited message. | Target B followed by a no-link edit; query must show zero evidence rows for that message. | Reversing edit/delete ordering or leaving evidence from the previous revision. |
| The same URL in visible text and a hidden Telegram link keeps provenance while representing one target identity. | Both sources point to `https://example.invalid/shared`; SQL must contain two source rows but one distinct hash, matching the independent expected digest twice. | Source-neutral aggregation creates duplicate resources, or a hidden target is replaced by its visible label. |
| QR results can only finalize for the current media generation. | Persist photo generation A, lease its job, replace stored raw message with photo B, then try to write A's QR target before queue supersession. Result must be `None` and zero QR evidence rows remain. | Remove the current-media/file-unique-ID generation check: the stale write returns `Some(1)`, and the regression fails. This mutation was run and caught. |
| `ingest=false` has no persistence side effects. | Synthetic message with user and URL against a test config whose chat has ingest disabled; `ingest_message` must return `false`, with zero message, profile, and URL-evidence rows. | Call `save_telegram_message` before checking the ingest gate or persist secondary projections despite disabled ingest. |
| Instance databases do not share link evidence, profiles, notes, or messages. | Two synthetic DB scopes use unique markers; each MCP client can read only its own profile/notes/messages, and direct SQL checks find each URL-evidence marker only in its own DB. | Reuse one DB/manifest or cross-wire instance connection/scope. |

The pure reference model and generator live under `tests/support/quality`. Each trace is scoped by `(instance, chat, user, message)` and may contain receive, duplicate, edit, link removal, QR attach, media replacement, shadow refresh, stale retry, and delete operations. The model checks duplicate/stale no-ops, latest revision, QR media generation, shadow revision, and instance-key separation. It does not call production SQL or URL parsing functions. The SQL assertions use persisted rows as the oracle.

The generated trace property runs 256 cases with shrinking. Default replay seed is `72623859790382856` (`0x0102030405060708`); failure includes case ID and replay command. Reproduce it with:

```sh
sh scripts/replay_quality_case.sh ingress-reference-trace 72623859790382856
```

## Verification

- `cargo fmt --all -- --check` — passed.
- `cargo test --workspace --all-targets --locked -- --quiet` — 684 passed, 0 failed, 13 ignored.
- `CARGO_BUILD_JOBS=3 CARGO_PROFILE_TEST_DEBUG=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- Seeded model test with default features — 1 passed / 256 traces.
- Seeded model test with `--no-default-features` — 1 passed / 256 traces.
- URL evidence PostgreSQL lane — 1 passed; includes duplicate/edit/remove, multi-source target, and stale QR media finalization.
- `scripts/test.sh` — passed, including the disabled-ingest PostgreSQL assertions and two-database MCP isolation test with URL evidence markers.
- Deliberate mutation removing the current media-generation check — PostgreSQL regression failed as expected (`stale_write` was `Some(1)` instead of `None`); mutation was reverted and the correct implementation passed.

No production spam scoring, moderation action, prompt, or model was changed. No live chats, production DB, private exports, user URLs, or external services were used. PostgreSQL tests only connected to the local disposable databases created by `scripts/test.sh`; they use `example.invalid` synthetic targets.
