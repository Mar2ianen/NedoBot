# T10 — MCP read-model, SQL scope and database isolation

## Invariants and counterexamples

| Invariant | Independent oracle / minimal case | Mutation or defect caught | Layer |
|---|---|---|---|
| Filter values never become SQL syntax. | Seeded SQL-like strings are supplied as valid text equality filters. The independent oracle requires the exact string in the bind vector, a `$1::text` placeholder in SQL, and no payload substring in SQL. | Mutating the filter builder to inline values failed at seed `12345`; shrink: `' OR 1=1 --`. | Query builder property test, 256 schema-valid cases. |
| Scope filters cannot include any foreign chat/channel, even when mixed with an in-scope ID. | Positive IDs are generated against synthetic group scope `-2001`; every forged value must fail. Existing boundary table checks exact match succeeds and `IN(own, foreign)` fails. | Mutating `.all(...)` to `.any(...)` failed on the mixed-channel-list case. | Pure scope policy plus property test. |
| Cursors preserve the numeric offset and malformed encodings fail closed. | 256 seeded nonnegative `i64` offsets round-trip; malformed base64 and wrong-width payloads are rejected. | Encoding/decoding off-by-one or accepting truncated cursor bytes. | Query module property test. |
| A community MCP client cannot read a second community's messages, notes, or profiles. | Existing test creates two disposable PostgreSQL databases and distinct synthetic markers, reads own fixtures, asserts foreign markers absent, and rejects wrong database/manifest and secondary chat scope. | Removing/weakening database instance scope or manifest checks makes cross-read or startup assertions fail. | `tests/chat_db_mcp.rs::independent_community_databases_generate_manifests_and_start_isolated_mcp_servers`. |
| MCP credentials cannot write even if a handler regresses. | Existing disposable-PostgreSQL lane assumes `nedobot_mcp_ro`, verifies reviewed views are readable and base/private tables plus write operations are denied. | Granting write access or exposing raw tables violates ACL assertions. | Existing read-only role permission assertions in `tests/postgres_migrations.rs`. |

## Replay

```sh
sh scripts/replay_quality_case.sh mcp-sql-bindings 12345
sh scripts/replay_quality_case.sh mcp-forged-scope 12345
sh scripts/replay_quality_case.sh mcp-cursor-roundtrip 12345
```

Generated cases use fixed seeds, shrinking and synthetic values only. SQL interpolation failed at seed `12345` with `' OR 1=1 --`; changing the scope `all` check to `any` failed on the mixed own/foreign source-channel list; cursor `offset + 1` mutation failed at the minimal offset `0`. All mutations were restored.

## Scope and limitations

- The existing two-database test is the real PostgreSQL isolation oracle; no mock replaces scope locks, ACLs, manifests or SQL constraints.
- No production database, credentials, user DSN, Telegram chat, or external MCP/LLM endpoint was used.
- No new transport test was needed: `tests/rmcp_transport.rs` already covers local duplex, stdio-child and Streamable HTTP initialization, call and close paths. MCP resource enumeration is currently static catalog metadata; it exposes no user rows to page.
- These new properties cover query value binding, forged scope and cursor encoding. Broader tool argument-schema permutations remain covered by the existing request validators and integration calls.
- `docs/CODE_STYLE.md` and `docs/TEST_QUALITY.md` are absent in this checkout; root `AGENTS.md` and the archive T10 contract were used.

## Verification

- `RUST_TEST_THREADS=8 CARGO_BUILD_JOBS=3 sh scripts/test.sh` — 660 passed, 0 failed, 17 ignored across 46 executed test targets/lanes. The script ran disposable PostgreSQL note/CAS and read-only checks, `nedobot_mcp_ro` permissions, local MCP child startup, and two-database community isolation.
- Three new seeded property replays passed at seed `12345` (256 generated cases each).
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo check --no-default-features --lib --locked` and `git diff --check` — passed.
