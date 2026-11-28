# Conversation flood shadow

The first NedoBot integration is opt-in and observational. Build with
`--features flood` and set `FLOOD_SHADOW_ENABLED=true` to analyze a 15-minute
window after a message is saved or edited. The database query is bounded by
recent ingestion time; the detector window uses Telegram's event timestamp.
The default build and default runtime configuration do not run the observer. Setting the environment option without
the Cargo feature fails runtime validation.

The observer reads at most 2,049 current message rows from the existing
`telegram_messages` table for the same chat and forum topic. It keeps the
newest 2,048, uses the existing edit-updated JSON projection, and delegates bounded
feature calculation to `teloxide-antispam::flood`. Album items remain separate
media items but form one logical send. Text tokenization uses the existing
versioned antispam preprocessor. The adapter counts custom emoji entities; it
does not approximate native Unicode emoji recognition.

Each snapshot is marked incomplete. The current ingest table cannot prove that
all Telegram updates in the window were received (for example, while ingest was
disabled or during an outage). Metrics are emitted at trace level without
message text, file IDs, usernames, or a moderation decision. Flood observations
do not create spam labels, reviews, enforcement, or training data.

This is an integration slice, not a complete detector rollout. Durable event
idempotency across restarts, replay calibration, emoji registry/rules, visual
decoding, and MCP media access require separate reviewed work. No decoder,
second ASR queue, or new Telegram API client is introduced here.
