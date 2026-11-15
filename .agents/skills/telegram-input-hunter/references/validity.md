# Validity classes

- **V1:** Observed in an authorized real Telegram Update OR fully documented, Bot API-realizable construct with cross-field constraints verified against Bot API and project teloxide types. State which evidence establishes realizability.
- **V2:** Deserializes / conforms to a schema but is not verified to occur in Telegram delivery (e.g., contradictory optional fields, strange combinations). Valid *for parser fuzz*, not a confirmed Telegram-valid defect.
- **V3:** Violates Telegram schema or intentionally malformed. Still useful for no-panic/resource checks.

Always track counts by class. URL payloads may be syntactically valid but not provably deliverable in a given Update context. Entity offsets are UTF-16 code units; verify whole actual text/caption. Avoid unsupported assumptions about edited fields, forward combinations, bot-only messages and inaccessible messages.
