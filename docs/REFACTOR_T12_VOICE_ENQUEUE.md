# T12 — Voice to enqueue-only execution

## Scope

Baseline: T11 deterministic-test correction `a2b995c` (draft PR #144). Branch: `codex/refactor-t12-voice-enqueue-only`, stacked on T11.

## Behavior before and after

Previously a voice update handler performed create → claim → Telegram media download → ASR → cleanup → transcript delivery inline. Telegram did not process later updates on that dispatcher task until the entire audio operation returned. The `/transcribe` reply command used the same inline path. In the ordinary message path, auto-transcription ran after ingestion and `/userstats`, before optional first-comment handling; a recognized voice update consumed the message even when the job was already active or terminal.

Now the update handler recognizes and enqueues the source message, then returns a typed `VoiceEnqueueOutcome`. It does not receive a Bot handle on the auto-transcription path, and the shared `transcribe_media_message` helper has no Bot argument. Only the supervised voice worker claims the durable job and runs download, ASR, cleanup, progress rendering, and Telegram delivery. The worker poll remains the configured five-second cadence. A new job's initial progress message appears after a worker claims it, instead of being sent inline during the update.

## Changes

- `enqueue_voice_job()` uses `ON CONFLICT DO NOTHING`; a duplicate returns `AlreadyPresent` and can never reset a terminal job or create another job for the same `(chat_id, message_id)`. The worker independently reclaims eligible expired leases through the existing claim query.
- Manual `/transcribe` reply and automatic voice/audio/video-note handling share the enqueue path. Existing enablement, chat checks, command filtering, and voice-before-first-comment precedence remain intact. `/transcribe` validation errors still use the existing Telegram response; a valid request only writes the job.
- Existing progress message, raw ASR transcript, cleaned transcript, no-speech fallback, retry budget, `delivery_unknown` state, and expired-delivering recovery remain on the worker path unchanged.
- Claimed jobs now include their existing `created_at` timestamp. The worker logs queue latency in seconds and the existing recovery count, with only the durable job ID and numeric timing/count fields.
- Added duplicate/terminal PostgreSQL assertions, an assertion that enqueue leaves the job pending/unclaimed without a progress message, a queue-consumption precedence test, and a mock Telegram Bot API delivery test. Existing ASR tests use a local fake Groq HTTP server and verify multipart requests without contacting a provider.
- No migration or dependency added.

## Verification

Local:

- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `CARGO_TARGET_DIR=../nedobot-refactor-t11-wip/target cargo check --workspace --all-targets --locked` — passed.
- `CARGO_TARGET_DIR=../nedobot-refactor-t11-wip/target cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `cargo test --workspace --all-targets --locked` — CI passed: 610 passed, 0 failed, 6 ignored; additional test targets passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — CI passed.
- PostgreSQL migrations and configured integration tests — CI passed, including URL evidence/shadow checks, migration lifecycle, RMCP (2 tests), and manual moderation (3 tests).
- CI runs [#43817792737](https://github.com/Mar2ianen/NedoBot/actions/runs/43817792737) and [#43817799544](https://github.com/Mar2ianen/NedoBot/actions/runs/43817799544) passed on the final code head. The local disk had less than 1 GB free, so the full test-profile build was delegated to CI.

## Invariants and limitations

- ASR and media download no longer run on the Telegram update task. Telegram work is asynchronous after durable enqueue.
- Voice remains ahead of automatic first-comment handling for recognized voice updates, including duplicates. If enqueue itself fails, the existing main handler logs the failure and continues to later message handling.
- `AlreadyPresent` intentionally covers any existing source-message job, including pending, in-flight, and terminal records; it never offers an implicit retry of a terminal result.
- On restart, existing leases and `recover_expired_voice_deliveries()` retain recovery behavior. A stale `delivering` job is made `delivery_unknown`, not sent again.
- No production DB, live bot, external ASR provider, deployment, or merge was used.

## Review state

Draft PR [#145](https://github.com/Mar2ianen/NedoBot/pull/145) remains open and unmerged. Current head `b1983ac`; both PR checks passed. No deployment performed.
