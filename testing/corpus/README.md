# Corpus layout

`public/` contains synthetic examples only. They are input seeds and **not** a claim that each shape is a Telegram-server-emitted V1 Update. `synthetic_urls.jsonl` includes explicit metamorphic semantic expectations. `synthetic_updates.jsonl` includes illustrative Bot API shapes marked V2 until externally verified.

No `private/` directory is packaged. Keep real historical corpus **outside** repository and CI. Avoid attempting to sanitize private data just by removing usernames; URLs and timestamps are often identifying. See `testing/references/CORPUS_PRIVACY.md`.
