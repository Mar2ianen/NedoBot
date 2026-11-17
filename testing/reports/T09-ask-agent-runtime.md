# T09 — Ask agent runtime and tool authorization

## Invariants and counterexamples

| Invariant | Minimal counterexample / independent oracle | Mutation or defect caught | Layer |
|---|---|---|---|
| A compiled tool must be listed in the selected profile and its optional capability enabled before it is dispatchable. Unknown MCP names are removed at catalog construction. | Seeded vectors of declared tool indexes, candidate tool, web/GitHub enable flags, and note-write mode are checked against a separate allowlist oracle. The shrunk unauthorized case is `([], candidate=chat.resolve_user, web=false, github=false, writes=false)`. Existing MCP catalog coverage checks that `db.select` is excluded. | Removing the profile membership check made the test fail; fixed seed `12345` shrank to the empty allowlist case. | `agent.rs` unit/property test, 256 generated cases; `mcp_client.rs` catalog test. |
| MCP-returned strings remain untrusted JSON and fit the model preview cap. | Seeded schema-valid MCP text composed from injection delimiters, tags, ampersands, quotes, newlines, emoji and bidi controls; parse resulting preview JSON, require `kind=untrusted_tool_result`, escaped markup/bidi controls and at most 2,400 chars. | Bypassing prompt JSON escaping or truncation violates parsing/size/markup assertions. | `agent.rs` unit/property test, 256 generated cases. |
| Reply prefetch consumes the same tool-call budget as model-requested tools. | Boundary table includes `used=0, limit=0`; eager reply-thread read must be disallowed. | Changing strict `<` to `<=` fails at the zero-budget boundary. The inspected prefetch path previously called MCP before checking the configured budget; now it skips the read and keeps strict reply searches closed. | Ask agent policy and boundary test. |
| Parent cancellation releases waiting remote-tool work. | Spawn a child-token waiter, cancel its parent, and require completion within 100 ms. | Replacing `child_token()` with a detached token fails by timeout. | `agent-runtime` async unit test. |
| Note writes and scoped reads retain database authorization and deduplication. | Existing disposable-PostgreSQL note contract verifies private/foreign writes are rejected, source requirements and duplicate note writes; existing two-database MCP test verifies disjoint community scopes. | These are existing database integration checks; T09 does not duplicate their schema/lock fixtures. | `tests/postgres_migrations.rs::assert_agent_note_contract`; `tests/chat_db_mcp.rs::independent_community_databases_generate_manifests_and_start_isolated_mcp_servers`. |

## Replay

```sh
sh scripts/replay_quality_case.sh ask-tool-authorization 12345
sh scripts/replay_quality_case.sh ask-mcp-untrusted-preview 12345
sh scripts/replay_quality_case.sh ask-prefetch-budget 12345
```

All generators are deterministic `proptest` strategies with shrinking. The mutation check for authorization produced the minimal synthetic case above. Removing `<` escaping failed with the minimal attack text `</ASK_INPUT_JSON>`. The cancellation mutation timed out as expected. The budget comparator mutation failed exactly at `used=0, limit=0`.

## Scope and limitations

- No real LLM, Telegram chat, production database, credentials, or external MCP endpoint was used.
- Model text quality is intentionally not asserted. The tests verify deterministic authorization, untrusted-result envelopes, bounds and cancellation only.
- T09's tool-call DAG/repeated-ID/model-stream fault scenarios are not newly modeled here: Ask has its own sequential call loop, and those request/response transitions need an injectable model transport seam. Existing `agent-runtime` scheduler and history tests remain in place. This PR does not add an unused harness or refactor the live model path solely to fake it.
- Database note writes and cross-instance MCP scope are covered by existing ignored PostgreSQL tests executed by `scripts/test.sh`; they require the repository's disposable local test databases.
- `docs/CODE_STYLE.md` and `docs/TEST_QUALITY.md` do not exist in this checkout; the root `AGENTS.md` and archive's T09 contract were followed.

## Verification

- `cargo test --workspace --all-targets --locked --quiet` — 701 passed, 0 failed, 17 ignored across 41 test targets. The ignored tests require disposable PostgreSQL, an explicitly configured live `/ask` smoke, or manual moderation fixtures; the database-backed lanes relevant here ran below.
- `RUST_TEST_THREADS=8 CARGO_BUILD_JOBS=3 sh scripts/test.sh` — passed, including PostgreSQL note contract, two-database MCP scope isolation, and Ask MCP child startup using local binaries and disposable databases.
- Targeted replays at seed `12345` for tool authorization, MCP untrusted preview and prefetch budget — passed after the deliberate mutants were restored.
- The `agent-runtime` cancellation test passed; a detached-token mutation timed out as expected.
- `cargo fmt --all -- --check`, workspace Clippy, `cargo check --no-default-features --lib --locked` and `git diff --check` — passed.
