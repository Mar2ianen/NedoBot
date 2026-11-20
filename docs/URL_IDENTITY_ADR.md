# ADR: URL service identity and resource identity

**Status:** accepted for architecture only; no policy or moderation enforcement.

## Context

A Telegram message may refer to a service, a particular resource hosted by that service, an affiliated discussion/channel, or an external destination. These are different identities. A hostname alone is not a safe proxy for either identity, and URL evidence must remain private and source-neutral.

## Decision

- `ServiceId` means the provider/platform account or service. It can only be assigned from a future authoritative, typed registry; a hostname or URL path is not sufficient evidence.
- `ResourceId` means one exact provider resource, such as a particular product/listing, post, or video. It is scoped by its `ServiceId` and retains the service's canonical resource identifier. URL aliases can point to it only after a service-specific resolver proves equivalence.
- Current URL evidence keeps a canonical URL fingerprint and provenance (`visible_text`, Telegram entity, RichMessage, QR). It does not infer service/resource identity, follow redirects, fetch message-provided URLs, or treat host reputation as trust.
- Ozon regional storefront links are a future resolver candidate only. A product ID may identify a candidate resource across regions only after a documented resolver proves equivalence. Similar paths, shared hostnames, redirects, or an assumed cross-region URL mapping never establish equality.
- A future automatic-moderation decision must consume typed evidence and an explicitly authorized policy object. It must keep source, service, and resource evidence separate; distinguish unknown from negative; define freshness and conflict handling; include audit provenance; and remain opt-in behind owner-reviewed thresholds and human-labeled evaluation. This ADR does not authorize allow/deny lists, sanctions, or a change to the existing shadow-only URL signal.

## Consequences

No new IDs, resolver, dependency, database table, network request, allowlist, or denylist is introduced by this ADR. URL extraction continues to use the pinned `teloxide-url` parser.
