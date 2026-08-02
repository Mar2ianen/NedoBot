# Changelog

All notable changes to NedoBot are documented here.

## [Unreleased]

### Added

- `/ask` semantic Rich Text delivery through the shared teloxide pipeline;
- explicit time, trusted link-alias and custom-emoji bindings in one render context;
- `chat`, `message_<id>` and `source_N` aliases built only from observed or trusted application data;
- delivery certainty and immutable render audit fields for `captured_now`, dialect, timezone, renderer revision and compiled Markdown;
- provenance validation for Markdown links and bare URLs before final delivery;
- safe fallback policy: fallback is allowed only for `NotAttempted` or confirmed `Rejected`, and is suppressed for `Unknown` delivery;
- lifecycle previews through shared Drafter: native drafts in private chats and one editable Rich Message in groups.

### Fixed

- bounded URL-aware time scanning no longer treats ordinary dates, URI paths or code fragments as malformed markers;
- final and segment delivery errors retain conservative certainty instead of triggering unsafe retries or duplicate fallback messages;
- temporary progress previews are cleaned best-effort after confirmed final rejection;
- citations include bare Telegram message URLs as well as aliases and Markdown link destinations;
- unknown or untrusted literal URLs are rejected before Telegram delivery.

### Changed

- NedoBot pins the exact teloxide implementation used by the semantic Rich Text renderer;
- `/ask` stores source Markdown separately from compiled Markdown so delivered Telegram payloads can be replayed without reparsing with a newer renderer or timezone database;
- the current release candidate is covered by [teloxide PR #41](https://github.com/Mar2ianen/teloxide-fork/pull/41) and [NedoBot PR #11](https://github.com/Mar2ianen/NedoBot/pull/11).

### Verification

- teloxide head: `20269818ba27de4c010c9ff52ad2fd4d080403a3`;
- NedoBot head: `45d0ef298063ef903ac8d2d8a02c2abb397dca44`;
- teloxide CI: [30759297569](https://github.com/Mar2ianen/teloxide-fork/actions/runs/30759297569);
- NedoBot CI: [30759297818](https://github.com/Mar2ianen/NedoBot/actions/runs/30759297818);
- merge and deploy are intentionally not part of this release-candidate change.
