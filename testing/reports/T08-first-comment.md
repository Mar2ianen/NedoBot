# T08 — First-comment delivery, preview, idempotency and renderer safety

## Invariants and counterexamples

| Invariant | Minimal counterexample / independent oracle | Mutation or defect caught | Layer |
|---|---|---|---|
| Concurrent copies of the same discussion post create one semantic job. | 16 workers enqueue the same `(discussion_chat_id, discussion_message_id)`; PostgreSQL must contain one row and exactly one call returns an ID. | Remove `ON CONFLICT DO NOTHING`: test observes unique-key errors from concurrent duplicate events. | Disposable PostgreSQL. |
| Owner preview goes only to the configured owner destination and includes source identity. | Local fake Bot API captures `sendMessage` and checks `chat_id=OWNER_ID`, source message ID and preview text. | Route preview to the public discussion chat or omit owner-facing metadata. | Local fake Telegram endpoint. |
| Model-controlled strings never become trusted HTML markup. | Seeded input generator combines non-BMP Unicode, quotes, ampersands, `<script>`, HTML-looking tags, NUL/control characters and newlines; expected output has no literal `<` because the fixture has no trusted placeholders. | Insert model text with `Html::raw_trusted` or skip escaping. | Production renderer + 256-case shrinking property. |
| A sending job that may have reached Telegram is never automatically reclaimed; finalization is transactional and idempotent. | Existing PostgreSQL lifecycle transitions expired `sending` to `delivery_unknown`, rolls back a failed DB finalization, and rejects a second finalization. | Remove send fence, auto-retry unknown delivery, or partially persist sent state. | Existing disposable PostgreSQL lifecycle tests. |
| Unsafe or blocked external source links do not render as clickable URLs. | Existing independent unsafe-local and blocked-domain source fixtures must remain escaped placeholder text. | Skip safe-fetch or configured deny policy during final rendering. | Existing render unit tests. |

## Verification

- `owner_preview_uses_only_the_configured_private_destination` passed against a local fake Bot API.
- `concurrent_duplicate_post_comment_events_create_one_semantic_job` passed against disposable PostgreSQL with 16 concurrent enqueue workers.
- Deliberately removed `ON CONFLICT DO NOTHING`; the concurrency test failed with duplicate-key errors as expected. Mutation was restored.
- Seeded renderer generator: 256 cases; default seed `578437695752307201`; replay seed `12345` passed:

```sh
sh scripts/replay_quality_case.sh first-comment-render 12345
```

- Existing source allow/deny and send/finalize/uncertain-delivery tests remain in the standard suite.
- `RUST_TEST_THREADS=8 CARGO_BUILD_JOBS=3 sh scripts/test.sh` — all configured lanes passed, including the new PostgreSQL case, MCP isolation, voice lifecycle and moderation-race tests. One earlier unbounded-parallel run hit an intermittent fake-process spawn failure in the existing YouTube subtitle test; its isolated retry and the bounded-thread full run both passed.
- `CARGO_BUILD_JOBS=3 cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo fmt --all -- --check` and `git diff --check` — passed.
- No production posts, Telegram chats or external search/LLM endpoints were used.
