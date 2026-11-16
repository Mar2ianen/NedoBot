# T20 extension contract — hypothetical media import

This is a static design review. It adds no runtime downloader or media-import feature.

## Request and types

- `MediaImportRequest`: instance scope, actor, Telegram trigger identity, source URL, requested target chat, and stable request ID. The URL is input, never an authorization scope.
- `Source`: narrow async port that resolves/fetches a validated media resource into a bounded temporary object. A fake implementation supports tests.
- `Trigger`: command/update adapter that validates permissions, feature gate, and request size before persisting work.
- `UploadPlan`: verified metadata, bounded local artifact reference, destination and delivery-idempotency key. It carries no secret-bearing source URL into logs.
- `Failed` means a confirmed pre-send or Telegram rejection. `Unknown` means Telegram may have accepted the upload; it is persisted as terminal `delivery_unknown` and requires operator reconciliation.

## Expected change surface

| Area | Expected addition | Existing feature internals touched? |
|---|---|---|
| Telegram trigger | `commands.rs` enum variant and thin `command_handler.rs` dispatch adapter | No `handle_message` condition for a command trigger |
| Runtime config | typed `media_import` enablement, limits, executable policy and destination gate | No changes to Ask, voice, or audit config |
| Domain | `features/media_import/{types,policy,service,provider}.rs` | New isolated vertical slice |
| Durability | `features/media_import/repo.rs`, additive job migration with claim/lease/retry/terminal state | No reuse of another feature's tables |
| Startup | bounded worker registration in `main.rs` under compile-time and runtime gates | No changes to other worker loops |
| Metrics | named job/error/delivery-certainty counters in `features/jobs/observability.rs` | No shared untyped status strings |
| Tests | policy unit tests, fake source/delivery tests, migration lifecycle and retry/restart tests | No live service, private corpus, or model required |

## Boundaries and tradeoffs

- Keep two identities: a source-object cache key `(canonical resource, transform version)` and a per-request delivery key. Reusing bytes must not collapse independent requests or suppress a separate Telegram send.
- A subprocess, if eventually approved, has a fixed executable, no shell interpolation, argument allowlist, bounded stdout/stderr, timeout, concurrency semaphore, and process-group cleanup. A timed-out blocking process must not outlive its permit.
- Apply the existing URL parser/SSRF policy before fetch. Resolve and validate each redirect target; reject private, loopback, link-local and non-HTTP destinations. Do not assume DNS validation remains valid after redirect.
- Do not implement until source-service terms, copyright policy, retention, and user expectations are reviewed. Shorts/TikTok support is outside this refactor task.
- Telegram upload acceptance can be ambiguous. Never resend the full result automatically after `Unknown`; expose a reconciliation path instead.
- Existing central command registration is simple but not fully plugin-discoverable. This change surface is intentional and visible; avoid introducing a generic actor/ECS registry to conceal it.

## Static verification

This document adds no downloader code. Source/import scans and the absence of media-import modules, subprocess calls, Telegram send paths, and command variants are recorded in `docs/REFACTOR_T20_EXTENSIBILITY.md`.
