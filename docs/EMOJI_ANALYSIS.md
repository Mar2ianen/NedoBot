# Emoji analysis

The `emoji` Cargo feature enables deterministic extraction and descriptive
metrics for Unicode emoji and Telegram custom-emoji entities. It is opt-in and
is not part of the default release profile. Enabling it adds no worker, API
request, persistence, command, or moderation policy by itself.

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
custom-emoji entity at the same span and ID wins and records the fallback as
additional provenance. Plain text URLs, malformed candidates, invalid spans,
and non-emoji labels do not become custom occurrences. Validated text/caption
fallback-only targets are omitted from `message_url_targets` by exact ID, span,
and label correlation, while raw URL evidence remains available for audit.
RichMessage URL findings stay in URL targets until the adapter exposes a stable
per-node join key, avoiding suppression based only on a repeated label. This
is compatibility with a known client format, not a
claim that Telegram or every client guarantees it. No sanitized live Bot API
transport capture is available yet.

The output contains occurrences with UTF-8 and UTF-16 spans, identity and
format, plus aggregate counts, visible non-emoji graphemes, density, maximum
consecutive run, maximum repeats, and unique identities. It is evidence only;
callers must choose any interpretation or policy. Inputs are bounded to 1 MiB
of UTF-8 and 2,048 custom spans.

```sh
cargo test --no-default-features --features emoji --lib features::emoji_analysis --locked
```
