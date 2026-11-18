# T14 — URL, QR and subprocess resource boundaries

## Invariants, counterexamples and oracles

| Invariant | Minimal case / independent oracle | Defect the test catches | Layer |
|---|---|---|---|
| Unsafe IP literals never reach an MCP fetch request. | A mixed result set contains `http://[::1]/admin`, the IPv4 metadata address, and two public hostname lookalikes. `fetch_urls` must drop the first two and preserve the latter two. | Removing the fetch policy call, failing to parse bracketed IPv6, or using suffix matching for the metadata hostname forwards the wrong URL set. | Search MCP URL selection unit test. |
| IPv4-mapped private IPv6 is also rejected. | Finite URL table covers loopback, ULA, link-local, mapped loopback and a public IPv6 address. | Testing only IPv4 or only unbracketed IP text misses private IPv6 targets. | URL policy unit tests. |
| QR decode output remains deterministic and bounded on structurally valid inputs. | Seed `12345`, 128 generated grayscale PNGs, width/height 1–128, independently encoded; scan each image twice and enforce code/payload caps. | Unbounded candidate/payload output, nondeterministic de-duplication, or panic on generated bounded PNGs. | `nedobot-qr` integration property test with shrinking. |
| A timed-out subtitle provider does not survive or populate cache. | Local executable fake records its PID and replaces itself with `sleep 30`; service timeout is 500ms. Assert timeout, empty cache, and process exit. | Await-only timeout handling that leaves the child alive, or caching partial/failed output. | Linux subprocess integration test. |
| Media download writer accepts exactly the configured cap and rejects the next byte without retaining it. | A four-byte synthetic limit: write `abcd`, then `e`. | Off-by-one or append-before-check allows payload growth beyond budget. | Async writer unit test. |

## Replay and verification

```sh
NEDOBOT_PROPTEST_SEED=12345 sh scripts/replay_quality_case.sh fetch-url-ip-literals 12345
NEDOBOT_PROPTEST_SEED=12345 sh scripts/replay_quality_case.sh qr-decoder-resource 12345
NEDOBOT_PROPTEST_SEED=12345 sh scripts/replay_quality_case.sh mcp-youtube-timeout 12345
NEDOBOT_PROPTEST_SEED=12345 sh scripts/replay_quality_case.sh media-writer-boundary 12345
RUST_TEST_THREADS=8 CARGO_BUILD_JOBS=3 sh scripts/test.sh
```

- The bracketed IPv6 loopback regression failed before the fix: `http://[::1]/` was accepted. It is rejected after parsing the bracket-stripped IP literal and checking IPv6 loopback/private ranges plus IPv4-mapped addresses.
- All four targeted replays passed at seed `12345`; QR used 128 generated cases. The full local script reached its final two-database MCP isolation test successfully. `cargo test --all-targets` reported 646 passed / 0 failed / 13 ignored; `scripts/test.sh` also explicitly ran its disposable PostgreSQL, moderation, URL-evidence and MCP lanes (16 additional passing executions).
- `cargo fmt --all -- --check`, `CARGO_BUILD_JOBS=3 cargo clippy --workspace --all-targets --locked -- -D warnings`, `CARGO_BUILD_JOBS=3 cargo check --no-default-features --lib --locked`, and `git diff --check` passed after the implementation was corrected for Clippy.
- All PostgreSQL tests used the local disposable databases created by `scripts/test.sh`. No live URL, Telegram, external MCP, production database or model endpoint was contacted.

## Existing coverage and limits

- Existing tests continue to cover malformed subtitle VTT, duplicate cue text, provider failures, invalid YouTube hosts, timeout configuration, QR corrupt/oversized bytes, and stale QR evidence. The new cases target gaps without changing production thresholds or media policy.
- Search fetch is delegated to a configured MCP provider. NedoBot can filter URL syntax and IP literals before forwarding, but it cannot validate the provider's DNS resolution or each redirect hop. DNS rebinding and redirect-to-private-address behavior are therefore **not proven safe by this change**; the remote provider must enforce those controls, or fetching needs to move behind a local resolver/redirect-validation seam. No network resolver/server test was run because this checkout has no local fetch transport seam and the task forbids live URL requests.
- QR property inputs are bounded valid grayscale PNGs, not a broad JPEG/WebP corpus or oversized decompression bomb. The implementation's compressed-byte and pixel caps remain covered by existing boundary tests; generated cross-format coverage is still open.
- Process-leak assertion uses Linux `/proc`, so that test is Linux-only. Windows/macOS child-process semantics were not exercised.
- `docs/CODE_STYLE.md` and `docs/TEST_QUALITY.md` are absent in this checkout; repository `AGENTS.md`, task requirements and existing style/tests were followed.
