# T06 — Telegram delivery certainty and fallback

## Invariants and counterexamples

| Invariant | Minimal counterexample / oracle | Mutation or broken behavior caught | Layer |
|---|---|---|---|
| An unparseable response after a send is `Unknown`, because the API may have accepted it. | Fake local Bot API accepts a rich-send request and returns HTTP 200 with malformed JSON; the independent server call counter must stay at one. | Classify `InvalidJson` as a definitive reject and trigger a duplicate/fallback send. | Local fake HTTP API plus delivery classification. |
| Fallback is allowed only after a confirmed payload rejection. | Telegram `400 Bad Request: can't parse entities` allows a plain-text fallback; rate limit, permission errors and transport uncertainty do not. Existing fake verifies the exact request count and payload. | Fall back on timeout/unknown response, or stop after an explicit entity rejection. | Typed error table and fake API. |
| Durable first-comment recovery never automatically resends after uncertain delivery. | Existing disposable-PostgreSQL lifecycle checks move a sending lease to `delivery_unknown`; ordinary workers skip it and only an explicit operator action can retry. | Reclaim an expired sending lease as an ordinary retry, or auto-retry unknown delivery. | Existing PostgreSQL lifecycle oracle in `tests/postgres_migrations.rs`. |
| Voice fallback preserves certainty and stops after an already delivered preview. | Existing code classifies network/invalid JSON/I/O as unknown; document failure after preview keeps the sent preview and stops automatic resend. | Treat file upload uncertainty as safe to retry the whole transcript. | Voice classification tests and production delivery flow. |
| Telegram text limits count Unicode scalar values and reject only values beyond the bound. | Exactly 4096 emoji and exactly 32768 rich-text emoji are accepted; one additional emoji is rejected. Empty input normalizes to a non-empty safe response. | Count UTF-8 bytes or allow an over-limit request. | Pure renderer boundary tests. |

## Confirmed fix

`delivery_outcome_is_unknown()` previously handled network and I/O errors but omitted `RequestError::InvalidJson`. Reports and spam-review delivery use this helper, while first-comment and voice pipelines already treated malformed acknowledgements as unknown. A focused regression failed before the fix and passed after adding `InvalidJson` to the unknown set. Fix commit: `48c1560` (`fix: treat malformed Telegram acknowledgements as unknown`).

## Verification

- The regression test was run before the fix and failed on `delivery_outcome_is_unknown(&malformed_ack)`; it passes after the fix.
- A local fake Telegram request returning malformed JSON observed one send only; the sender returned an error classified as unknown and did not issue a second send.
- Rich-message explicit payload-rejection fallback tests and voice classification tests are retained and run with the module test suite.
- Unicode boundary tests cover 4096 normal message characters and 32768 rich message characters using non-BMP text.
- Disposable PostgreSQL first-comment delivery lifecycle, operator-only uncertain retry, and finalization checks are already run by `scripts/test.sh` from T05.

No live Telegram calls or external endpoints were used. No delivery retry policy was broadened; the change only prevents misclassifying ambiguous acknowledgement parsing as a definitive rejection.
