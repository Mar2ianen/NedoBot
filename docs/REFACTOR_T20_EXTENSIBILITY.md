# T20 — Extension proof without a downloader

Baseline: T19 final published checkpoint `7ea9a4d` / draft PR #152. Branch: `codex/refactor-t20-extensibility`; PR metadata is recorded after publication. This walkthrough is static; it adds no executable downloader, command, Telegram send, ffmpeg, yt-dlp, or runtime configuration.

The hypothetical media import surface and its tradeoffs are in [`verification/EXTENSIBILITY_CONTRACT.md`](../verification/EXTENSIBILITY_CONTRACT.md). The expected change is a vertical feature slice plus the existing explicit command, config, worker, metrics, migration, and test registration seams. A command trigger does not require editing ordinary-message routing. None of `ask/agent.rs`, `voice/pipeline.rs`, or `new_user_analysis.rs` needs to change.

Tradeoffs recorded: separate source cache and delivery idempotency keys; typed `Failed`/`Unknown` delivery certainty; bounded subprocess and temp-file lifecycle; SSRF validation on every redirect; source terms/legal review before implementation. The current codebase is intentionally a modular monolith rather than a plugin loader.

Verification: `git diff 7ea9a4d --name-only -- src` is empty; `rg -n 'yt-dlp|ffmpeg|media_import|MediaImport|SendDocument|send_document' src` finds only existing URL/voice or generic Telegram send infrastructure. The T20 diff contains no runtime source changes. This is a static architecture review, so executable-test count is N/A; the unchanged T19 code passed its recorded full test matrix. No dependencies or schema changes.
