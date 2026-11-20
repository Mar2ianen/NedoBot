# T07 — Voice download, ASR and delivery boundaries

## Invariants and counterexamples

| Invariant | Minimal counterexample / independent oracle | Broken implementation caught | Layer |
|---|---|---|---|
| Actual downloaded media bytes never exceed the configured cap. | Telegram advertises a 1-byte file (or message metadata omits size) and streams 10 bytes; a 4-byte writer cap must stop the download. The local filesystem length must remain exactly 4 bytes. | Trust only Telegram's optional/advisory file size and write the full streamed body to disk. | Local fake Telegram API + bounded `AsyncWrite` file fixture. |
| Duration and declared-size validation accepts the exact bound and rejects only values above it. | Seeded schema-valid metadata varies each limit by -1/0/+1, and independently computes the expected decision. | Off-by-one comparisons or incorrect MB-to-byte conversion. | Fixed-seed property test with shrinking. |
| Duplicate Telegram media events do not create independent jobs; unknown delivery is never auto-replayed. | PostgreSQL enqueue twice for the same `(chat_id,message_id)` returns the same job ID; existing lifecycle checks transition `delivering → delivery_unknown` and claim none. | Remove the unique-key idempotency or automatically reclaim unknown delivery. | Disposable PostgreSQL. |
| Empty/no-speech output does not fabricate text; actual short speech remains present. | Existing unit cases distinguish silence/artifact strings from real one-word speech. | Drop real short speech or promote ASR artifacts to a transcript. | Structured synthetic transcript unit cases. |

## Confirmed fix

The download previously trusted message metadata and wrote the full Telegram response to an unbounded tempfile. When `file_size` was absent or understated, the configured voice file limit did not cap actual disk use. The regression was run before the fix and downloaded all 10 synthetic bytes despite an expected 4-byte ceiling. `download_media_file` now wraps its destination in a streaming byte-limited writer, supplied the configured maximum from the worker and `voice_smoke`; a response crossing the cap returns an error after at most the allowed prefix is persisted.

## Verification

- Local fake Bot API over loopback exercised `getFile` and a file body larger than its advertised size.
- Writer test proves a 10-byte write against a 4-byte limit errors and leaves exactly 4 bytes on disk.
- Regression failed before the fix: the fake download returned all 10 bytes against the 4-byte expectation. It passes after the streaming cap.
- Seeded generator: 256 media-boundary cases. Default seed: `506097522914230528`; explicit replay seed `12345` also passed:

```sh
sh scripts/replay_quality_case.sh voice-media-bounds 12345
```

- `CARGO_BUILD_JOBS=3 cargo test --lib features::voice -- --nocapture` — 33 passed, 0 failed.
- `CARGO_BUILD_JOBS=3 sh scripts/test.sh` — passed the full default-feature suite, voice lifecycle PostgreSQL scenario (including duplicate enqueue and uncertain-delivery no-retry), moderation races, MCP isolation and all configured DB lanes.
- `CARGO_BUILD_JOBS=3 cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `CARGO_BUILD_JOBS=3 cargo check --no-default-features --lib --locked` — passed; the voice-disabled compile path is clean.
- `cargo fmt --all -- --check` and `git diff --check` — passed.
- No Telegram production calls, ASR service, or external model was used.
