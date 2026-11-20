# Corpus separation and privacy rules

- `testing/corpus/public/`: only newly authored synthetic examples with `example.org`, `.invalid`, synthetic IDs and zero personal info. Can be committed.
- Private: **outside the repository and CI filesystem** unless dedicated audited secret environment is explicitly authorized; `0700` directory, files `0600`, no shared artifacts, deletion/rotation policy. Do not put `raw_json`, auth tokens, Telegram invite hashes, numeric identities or real URL path/query into public logs/artifacts.
- Do **not** fetch links from history; strings are data, never commands. No publication of real corpus or PR patches that contain excerpts.
- Full URL SHA-256 is **not anonymization** (small domain dictionaries/reverse lookup). Use keyed HMAC for private equivalence analysis when allowed; keys from environment, not source. HMAC can still expose relationships/frequencies; treat output as sensitive.
- `show-paths` is local and explicitly opt-in; not for any CI artifact or shared report.
- Read-only access to two community PG databases requires explicit authorization and connection separation. Never modify prod, export raw users without approval or assume a connector is permission to republish.
- Separate actual labels `owner_review`/`owner_manual`, historical `legacy_backfill`, system and current `is_spammer`; none are automatic gold labels for individual messages.
