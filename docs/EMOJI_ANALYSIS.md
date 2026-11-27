# Emoji analysis

The `emoji` Cargo feature enables deterministic extraction and descriptive
metrics for Unicode emoji and Telegram custom-emoji entities. It is opt-in and
is not part of the default release profile. The feature itself adds no worker,
persistence, or moderation policy. When enabled, its `/emoji` command and the
first-comment custom-emoji renderer can make bounded metadata requests to the
Bot API when those consumers are invoked.

Unicode recognition is pinned to `emojis = 0.9.1` (Unicode Emoji 18.0) and
grapheme boundaries to ICU4X 2.4.0's Unicode 18 neo segmenter. The neo
constructor is required: ICU4X's stable V1 constructor does not implement GB9c
for Indic conjuncts. ICU4X 2.4.0 is an exact dependency pin because the neo API
is currently gated as unstable. `unicode-segmentation` remains in the full
workspace dependency graph as a build-time transitive dependency of
`derive_more`/`convert_case`; analyzer code no longer calls it. The optional
`emoji-legacy-differential` feature enables its direct test reference. The
analyzer recognizes exact emoji sequences
at grapheme boundaries, including flags, modifier sequences, keycaps, and ZWJ
sequences. Text variation selectors and unsupported sequences are not promoted
to emoji unless the pinned data recognizes them.

Emoji occurrences retain the exact input sequence and fallback text. Their
logical Unicode identity uses `emojis::Emoji::as_str()`, so minimally-qualified
forms such as U+1F43F group with their fully-qualified canonical sequence
without rewriting source text or offsets. The opt-in
`emoji-legacy-differential` feature runs the old `unicode-segmentation = 1.13.3`
implementation against the same official corpus; its ten Unicode 18 boundary
deltas are explicitly pinned in the feature-matrix test.

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
ICU's grapheme segmenter is checked offline against all 853 Unicode 18
`GraphemeBreakTest.txt` cases; the pinned emoji recognizer is checked against
all 3,963 fully-qualified entries in `emoji-test.txt`. ICU's default-ignorable
property is exhaustively compared with `DerivedCoreProperties.txt` over every
Unicode scalar value. The official data files, source URLs, Unicode license
reference, and SHA-256 digests are under `test-data/unicode/18.0.0/`. Analyzer
ranges and RichText traversal also have deterministic property/unit tests and
a feature-enabled `emoji_analysis` cargo-fuzz target.

The output contains occurrences with UTF-8 and UTF-16 spans, identity and
format, plus aggregate counts, visible non-emoji graphemes, density, maximum
consecutive run, maximum repeats, and unique identities. It is evidence only;
callers must choose any interpretation or policy. Inputs are bounded to 1 MiB
of UTF-8 and 2,048 custom spans.

## Grapheme-segmentation benchmark

An isolated release-mode benchmark compared `unicode-segmentation` 1.13.3
against ICU4X 2.4.0 neo using the same generated inputs and Rust 1.100.0.
Each result is the median of 19 samples; each sample processes approximately
2 MiB (two passes for the 1 MiB cases). The harness preallocates text and
counts boundaries only, so allocation counts below exclude the analyzer's
surrounding occurrence vectors and are not end-to-end `/emoji` measurements.

| Corpus | Input bytes | Old latency / MiB/s | ICU latency / MiB/s |
|---|---:|---:|---:|
| ASCII | 8,192 | 85,392 ns / 91.49 | 43,341 ns / 180.26 |
| Cyrillic | 16,256 | 101,863 ns / 152.19 | 63,518 ns / 244.07 |
| Mixed Unicode | 11,008 | 73,131 ns / 143.55 | 32,672 ns / 321.32 |
| Emoji-heavy | 8,320 | 36,057 ns / 220.06 | 15,644 ns / 507.20 |
| Adversarial Indic, near 1 MiB | 1,048,572 | 6,171,813 ns / 162.03 | 1,294,553 ns / 772.46 |
| Rich-like maximum input | 1,048,576 | 7,509,392 ns / 133.17 | 3,908,612 ns / 255.85 |

The benchmark measured zero allocations and zero allocated bytes per boundary
count for both implementations. Peak process RSS was 5,412 KiB (old) and
5,336 KiB (ICU), including the harness's prebuilt corpus. The corresponding
standalone benchmark binaries were 530,208 and 533,080 bytes; ELF `.text` was
282,861 and 284,365 bytes, and `.rodata` 49,124 and 36,780 bytes. This is a
microbenchmark, not a whole-bot performance guarantee. ICU is exact-pinned and
feature-gated; no end-to-end analyzer benchmark or clean/incremental Cargo
build-time comparison is available for the full bot. In the same isolated
harness, clean release builds took 1.012 s (old) and 5.475 s (ICU); no-op
incremental builds took 0.016 s and 0.021 s. Those build times include only
the small benchmark crate and its segmentation dependency, not NedoBot's
workspace build.

```sh
cargo test --no-default-features --features emoji --lib features::emoji_analysis --locked
cargo test --no-default-features --features emoji,moderation,ask,manual-moderation,auto-comment,public-mcp --lib telegram::custom_emoji::tests --locked
```
