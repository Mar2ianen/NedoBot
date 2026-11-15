# V1/V2/V3: do not confuse parser-valid with Telegram-realizable

**V1:** observed in an authorized real update (with origin/trust verified) or demonstrably supported by Bot API spec including cross-field rules; link exact spec/behavior evidence. **V2:** parsed into teloxide or conforms to basic schema but reachability not confirmed. **V3:** invalid/corrupt payload; useful for bounded robust handling only.

Record: `validity_class`, `evidence_source`, `Bot API version`, `teloxide revision`, `cross_field_assumptions`. For UTF-16 entities calculate offsets/lengths using code units, not Rust char indices or UTF-8 bytes. For synthetic message fixtures without verified real-world reachability default to V2. Invalid Update variants should never be claimed a confirmed Telegram exploit.
