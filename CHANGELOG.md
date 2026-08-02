# Changelog

All notable changes to NedoBot are documented here.

## [Unreleased]

### Added

- `/ask` semantic Rich Text delivery through the shared teloxide pipeline;
- explicit time, trusted link-alias and custom-emoji bindings in one render context;
- `chat`, `message_<id>` and `source_N` aliases built only from observed or trusted application data;
- delivery certainty and immutable render audit fields for `captured_now`, dialect, timezone, renderer revision and compiled Markdown;
- provenance validation for Markdown links and explicit-scheme bare URLs before final delivery;
- safe fallback policy: fallback is allowed only for `NotAttempted` or confirmed `Rejected`, and is suppressed for `Unknown` delivery;
- lifecycle previews through shared Drafter: native drafts in private chats and one editable Rich Message in groups.
- durable voice transcription jobs with leases, bounded retries, recovery and CAS-guarded stage transitions;
- RMCP child-process transport for external search instead of a hand-written JSON-lines protocol.

### Fixed

- bounded URL-aware time scanning no longer treats ordinary dates, URI paths or code fragments as malformed markers;
- final and segment delivery errors retain conservative certainty instead of triggering unsafe retries or duplicate fallback messages;
- temporary progress previews are cleaned best-effort after confirmed final rejection;
- citations include bare Telegram message URLs as well as aliases and Markdown link destinations;
- unknown or untrusted literal URLs are rejected before Telegram delivery.
- ordinary dotted text is no longer classified as a bare URL; only explicit-scheme URLs enter provenance validation.
- reaction import rejects totals outside the PostgreSQL integer range instead of truncating them.
- literal LIKE filters escape `%`, `_` and `\\` while preserving case-sensitive and case-insensitive matching.

### Changed

- NedoBot pins the exact teloxide implementation used by the semantic Rich Text renderer (`40d9041d3b4eac1aa60dd10259ab688882be36ab`);
- `/ask` stores source Markdown separately from compiled Markdown so delivered Telegram payloads can be replayed without reparsing with a newer renderer or timezone database;
- the current release candidate is covered by [teloxide PR #41](https://github.com/Mar2ianen/teloxide-fork/pull/41) and [NedoBot PR #11](https://github.com/Mar2ianen/NedoBot/pull/11).

### Verification

- teloxide implementation revision pinned by `Cargo.toml`: `40d9041d3b4eac1aa60dd10259ab688882be36ab`;
- teloxide documentation/PR review head and CI run IDs are intentionally kept in PR #41/#11 rather than this version-controlled changelog;
- merge and deploy are intentionally not part of this release-candidate change.
