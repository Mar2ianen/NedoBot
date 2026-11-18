# Independent state-machine spec

Define synthetic state keyed by `(instance_id, chat_id, user_id, message_id)`:

```rust
// Sketch only, Codex must adapt to actual runtime and avoid production helpers in oracle.
enum Op {
    SaveMessage { chat: i64, user: i64, msg: i32, text: String, media_generation: u32 },
    EditMessage { chat: i64, msg: i32, text: String, media_generation: u32 },
    DeleteStoredMessage { chat: i64, msg: i32 }, // internal DB event, NOT Telegram Update
    ClaimQrJob { chat: i64, msg: i32, generation: u32 },
    CompleteQrJob { chat: i64, msg: i32, generation: u32, urls: Vec<String> },
    CancelWorker,
    RetryJob,
    RefreshShadow { chat: i64, user: i64 },
    ObserveUsername { user: i64, username: String, revision: u64 },
    ObserveMembership { user: i64, member: bool, revision: u64 },
    RefreshFeed { revision: u64 },
    RestartWorker,
}
```

Model logical authoritative content and QR generation; ignore stale result if generation mismatches; expected shadow derives from current message state. Use explicit quiescence transitions to flush pending jobs. Compare persisted projections at deterministic checkpoints. Old message arrival/order and crash may be internal simulation; distinguish from Telegram event validity.

Scheduler must be deterministic or controlled via failpoints/barriers. Tests relying solely on sleeps are timing dependent. Shrink operation **sequence** and **payload**. Re-run stored seed in PR CI.
