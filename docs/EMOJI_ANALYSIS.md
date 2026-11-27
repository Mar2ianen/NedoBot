# Emoji analysis

The `emoji` Cargo feature enables deterministic extraction and descriptive
metrics for Unicode emoji and Telegram custom-emoji entities. It is opt-in and
is not part of the default release profile. The feature itself adds no worker,
persistence, or moderation policy. When enabled, its `/emoji` command and the
first-comment custom-emoji renderer can make bounded metadata requests to the
Bot API when those consumers are invoked.

Unicode recognition is pinned to `emojis = 0.9.1` (Unicode Emoji 18.0) and
grapheme boundaries to `unicode-segmentation = 1.13.3`. The analyzer recognizes
exact emoji sequences at grapheme boundaries, including flags, modifier
sequences, keycaps, and ZWJ sequences. Text variation selectors and unsupported
sequences are not promoted to emoji unless the pinned data recognizes them.

Telegram custom entities are supplied as UTF-16 ranges. The adapter checks
addition overflow, text bounds, scalar boundaries, grapheme boundaries, and
overlap before analysis. A custom entity is counted once even when its fallback
text itself looks like Unicode emoji. Until a metadata lookup resolves the
sticker, its format is reported as unknown.

With the opt-in `emoji` feature, an addressed `TextLink` or rich `Url` node
using the exact client compatibility URI `tg://emoji?id=<positive decimal
u64>` is counted as a custom occurrence only when its complete label is one
recognized Unicode emoji grapheme. IDs are canonicalized as decimal strings;
the occurrence records `CustomUrlFallback` encoding provenance. A native
custom-emoji entity at the same span and ID merges with the URL evidence as
`NativeWithUrlFallback`; duplicate URL-only evidence remains URL-only. Plain
text URLs, malformed candidates, invalid spans, and non-emoji labels do not
become custom occurrences. Validated text/caption URL fallback targets,
including a matching native entity at the same ID and span, are omitted from
`message_url_targets` by exact ID, span, and label correlation, while raw URL
evidence remains available for audit.
RichMessage URL findings stay in URL targets until the adapter exposes a stable
per-node join key, avoiding suppression based only on a repeated label. This
is compatibility with a known client format, not a
claim that Telegram or every client guarantees it. No sanitized live Bot API
transport capture is available yet.

## Consumer commands

`/emoji_ids` replies with the custom emoji IDs in the addressed message. Its
report is bounded to 3,500 UTF-16 units and shows an omitted count when the
source has more occurrences. `/emoji` is available only with the `emoji` Cargo
feature; reply to a message to have the bot resolve and send premium custom
emoji entities. Resolution keeps occurrence order and duplicates, limits API
metadata lookup to the first 200 distinct IDs, and limits the outgoing message
to 100 entities and 2,800 UTF-16 units before its explanatory suffix. The
resolver returns per-ID outcomes for resolved, confirmed-missing, transient API
failure, and local lookup-busy states; `/emoji` reports retryable outcomes
separately from confirmed absence. The bounded metadata cache distinguishes
positive, confirmed-missing, and transient-error entries, while each call
retains its own positive results even if the cache evicts them. First-comment
rendering remains best effort and records deferred/transient lookup at debug
level.

`/qr` is registered only with `moderation` and still requires the per-instance
QR evidence setting. It accepts supported, unprotected image replies from the
same chat only when the stored media snapshot is present and matches both
message scope and file unique ID before download and again before display. A
two-slot admission limit bounds concurrent manual downloads; the command and
background worker share one bounded download/decode helper. Each image and
decode also has independent byte, timeout, and CPU limits. Raw QR payload bytes
are not logged or persisted by the command. Link-like payloads are displayed
as reversible UTF-8 hex without previews, so the report does not create a
clickable URL or rely on Unicode punctuation substitutions. Payload text that
cannot fit completely is reported as omitted with its byte size rather than
being shown as a truncated URL.

The feature matrix runs consumer tests with the combined supported feature
set, covering command registration, `/emoji` output and metadata resolution,
and `/qr` decoding/report helpers in addition to the pure analyzer tests.
Analyzer ranges and RichText traversal have deterministic property/unit tests;
there is not yet a standalone feature-enabled cargo-fuzz target for these
paths. Existing URL/history fuzz targets do not exercise the emoji analyzer.

The output contains occurrences with UTF-8 and UTF-16 spans, identity and
format, plus aggregate counts, visible non-emoji graphemes, density, maximum
consecutive run, maximum repeats, and unique identities. It is evidence only;
callers must choose any interpretation or policy. Inputs are bounded to 1 MiB
of UTF-8 and 2,048 custom spans.

```sh
cargo test --no-default-features --features emoji --lib features::emoji_analysis --locked
cargo test --no-default-features --features emoji,moderation,ask,manual-moderation,auto-comment,public-mcp --lib telegram::custom_emoji::tests --locked
```
