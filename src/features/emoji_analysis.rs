//! Optional, deterministic Unicode/custom emoji extraction and metrics.
//!
//! Recognition uses the exact Unicode data pinned by `emojis`; this module
//! does not fetch metadata, decode media, or apply moderation policy.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use icu_properties::{CodePointSetData, props::DefaultIgnorableCodePoint};
use icu_segmenter::{GraphemeClusterSegmenter, GraphemeClusterSegmenterBorrowed};
use teloxide::types::{CustomEmojiId, MessageEntity, MessageEntityKind, Utf16OffsetMap};
use teloxide_url::TelegramLink;

// ICU4X's V1 constructor does not implement GB9c (Indic conjuncts). The pinned
// 2.4.0 neo segmenter passes the full Unicode 18 GraphemeBreakTest corpus.
static GRAPHEME_SEGMENTER: GraphemeClusterSegmenterBorrowed<'static> =
    GraphemeClusterSegmenter::new_neo();

fn grapheme_indices(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut boundaries = GRAPHEME_SEGMENTER.segment_str(text);
    let mut start = boundaries.next();
    std::iter::from_fn(move || {
        let current_start = start?;
        let end = boundaries.next()?;
        start = Some(end);
        Some((current_start, &text[current_start..end]))
    })
}

fn graphemes(text: &str) -> impl Iterator<Item = &str> {
    grapheme_indices(text).map(|(_, grapheme)| grapheme)
}

pub(crate) fn is_single_grapheme(text: &str) -> bool {
    graphemes(text).take(2).count() == 1
}

/// Unicode emoji data version provided by the pinned recognition dataset.
pub const UNICODE_EMOJI_VERSION: &str = "18.0";

pub const MAX_TEXT_BYTES: usize = 1_048_576;
pub const MAX_CUSTOM_EMOJI_SPANS: usize = 2_048;
pub const MAX_CUSTOM_EMOJI_ID_BYTES: usize = 256;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum EmojiIdentity {
    Unicode(String),
    Custom(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmojiFormat {
    Unicode,
    Static,
    AnimatedTgs,
    VideoWebm,
    Unknown,
}

/// Media format of a Telegram custom emoji. Unicode is excluded because it is
/// represented by `EmojiKind::Unicode`, so a custom occurrence cannot carry
/// the impossible `format = Unicode` state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CustomEmojiFormat {
    Static,
    AnimatedTgs,
    VideoWebm,
    Unknown,
}

/// How a logical emoji was represented in the message. This is independent of
/// media format and preserves native plus fallback provenance when both occur.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmojiEncoding {
    Unicode,
    NativeCustom,
    NativeCustomWithUrlFallback,
    CustomUrlFallback,
}

impl EmojiEncoding {
    pub fn has_custom_url_fallback(self) -> bool {
        matches!(
            self,
            Self::NativeCustomWithUrlFallback | Self::CustomUrlFallback
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CustomEmojiProvenance {
    Native,
    NativeWithUrlFallback,
    UrlFallback,
}

impl CustomEmojiProvenance {
    /// Union two independently observed representations of the same custom
    /// emoji occurrence. Repeating a URL fallback cannot fabricate native
    /// entity provenance.
    pub fn merge(self, other: Self) -> Self {
        use CustomEmojiProvenance::{Native, NativeWithUrlFallback as Both, UrlFallback as Url};
        match (self, other) {
            (Native, Url) | (Url, Native) | (Both, _) | (_, Both) => Both,
            (Native, Native) => Native,
            (Url, Url) => Url,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EmojiKind {
    Unicode {
        sequence: String,
    },
    Custom {
        id: CustomEmojiId,
        format: CustomEmojiFormat,
        provenance: CustomEmojiProvenance,
    },
}

impl EmojiKind {
    pub fn identity(&self) -> EmojiIdentity {
        match self {
            Self::Unicode { sequence } => EmojiIdentity::Unicode(canonical_emoji(sequence)),
            Self::Custom { id, .. } => EmojiIdentity::Custom(id.0.clone()),
        }
    }

    pub fn format(&self) -> EmojiFormat {
        match self {
            Self::Unicode { .. } => EmojiFormat::Unicode,
            Self::Custom { format, .. } => match format {
                CustomEmojiFormat::Static => EmojiFormat::Static,
                CustomEmojiFormat::AnimatedTgs => EmojiFormat::AnimatedTgs,
                CustomEmojiFormat::VideoWebm => EmojiFormat::VideoWebm,
                CustomEmojiFormat::Unknown => EmojiFormat::Unknown,
            },
        }
    }

    pub fn encoding(&self) -> EmojiEncoding {
        match self {
            Self::Unicode { .. } => EmojiEncoding::Unicode,
            Self::Custom { provenance, .. } => match provenance {
                CustomEmojiProvenance::Native => EmojiEncoding::NativeCustom,
                CustomEmojiProvenance::NativeWithUrlFallback => {
                    EmojiEncoding::NativeCustomWithUrlFallback
                }
                CustomEmojiProvenance::UrlFallback => EmojiEncoding::CustomUrlFallback,
            },
        }
    }
}

/// A Telegram custom emoji's UTF-16 range and optional known representation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustomEmojiInput {
    pub id: CustomEmojiId,
    pub utf16_range: Range<usize>,
    pub format: CustomEmojiFormat,
    pub provenance: CustomEmojiProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmojiOccurrence {
    pub kind: EmojiKind,
    pub fallback: String,
    pub utf8_range: Range<usize>,
    pub utf16_range: Range<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmojiMetrics {
    pub unicode_count: u32,
    /// Unicode emoji recognized for moderation after safe ignorable handling.
    pub moderation_unicode_count: u32,
    pub custom_count: u32,
    pub animated_count: u32,
    pub unknown_format_count: u32,
    /// Non-emoji graphemes before default-ignorable filtering.
    pub visible_non_emoji_graphemes: u32,
    /// Non-emoji graphemes visible to moderation after default-ignorable filtering.
    pub moderation_visible_non_emoji_graphemes: u32,
    pub max_consecutive_run: u32,
    pub moderation_max_consecutive_run: u32,
    pub max_repeat_count: u32,
    pub moderation_max_repeat_count: u32,
    pub unique_count: u32,
    pub moderation_unique_count: u32,
    /// Emoji density over raw non-whitespace graphemes.
    pub density: Option<f64>,
    /// Emoji density excluding graphemes made only of default-ignorable code points.
    pub moderation_density: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmojiAnalysis {
    pub occurrences: Vec<EmojiOccurrence>,
    pub metrics: EmojiMetrics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmojiAnalysisError {
    TextTooLong,
    TooManyCustomEmojiSpans,
    CustomEmojiIdTooLong,
    EmptyCustomEmojiId,
    EmptyCustomEmojiRange,
    InvalidUtf16Boundary,
    InvalidGraphemeBoundary,
    InvalidGraphemeSegmentation,
    OverlappingCustomEmojiRanges,
    RichTraversalLimitExceeded,
}

/// Extracts exact Unicode emoji graphemes and typed Telegram custom emoji.
///
/// Custom entity ranges are validated before scanning. A custom emoji is
/// treated as one atomic emoji even when its visible fallback is ordinary
/// text or a Unicode emoji sequence, so it is never double-counted.
pub fn analyze(
    text: &str,
    custom: &[CustomEmojiInput],
) -> Result<EmojiAnalysis, EmojiAnalysisError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(EmojiAnalysisError::TextTooLong);
    }
    if custom.len() > MAX_CUSTOM_EMOJI_SPANS {
        return Err(EmojiAnalysisError::TooManyCustomEmojiSpans);
    }
    if custom
        .iter()
        .any(|item| item.id.0.len() > MAX_CUSTOM_EMOJI_ID_BYTES)
    {
        return Err(EmojiAnalysisError::CustomEmojiIdTooLong);
    }

    let mut custom_spans: Vec<(Range<usize>, &CustomEmojiInput)> = Vec::new();
    if !custom.is_empty() {
        let boundaries = Utf16OffsetMap::new(text);
        let grapheme_boundaries: HashSet<_> = grapheme_indices(text)
            .map(|(byte, _)| byte)
            .chain(std::iter::once(text.len()))
            .collect();
        custom_spans = custom
            .iter()
            .map(|item| {
                if item.id.0.is_empty() {
                    return Err(EmojiAnalysisError::EmptyCustomEmojiId);
                }
                if item.utf16_range.start >= item.utf16_range.end {
                    return Err(EmojiAnalysisError::EmptyCustomEmojiRange);
                }
                let byte_range = boundaries
                    .range(
                        item.utf16_range.start,
                        item.utf16_range.end - item.utf16_range.start,
                    )
                    .ok_or(EmojiAnalysisError::InvalidUtf16Boundary)?;
                if !grapheme_boundaries.contains(&byte_range.start)
                    || !grapheme_boundaries.contains(&byte_range.end)
                {
                    return Err(EmojiAnalysisError::InvalidGraphemeBoundary);
                }
                Ok((byte_range, item))
            })
            .collect::<Result<Vec<_>, _>>()?;
        custom_spans.sort_by_key(|(bytes, _)| bytes.start);
        if custom_spans
            .windows(2)
            .any(|spans| spans[0].0.end > spans[1].0.start)
        {
            return Err(EmojiAnalysisError::OverlappingCustomEmojiRanges);
        }
    }

    let mut result = EmojiAnalysis {
        occurrences: Vec::new(),
        metrics: EmojiMetrics::default(),
    };
    let mut counts = HashMap::<EmojiIdentity, u32>::new();
    let mut moderation_counts = HashMap::<EmojiIdentity, u32>::new();
    let mut run = 0u32;
    let mut moderation_run = 0u32;
    let mut byte = 0usize;
    let mut utf16 = 0usize;
    let mut custom_index = 0usize;

    while byte < text.len() {
        if let Some((range, input)) = custom_spans.get(custom_index)
            && range.start == byte
        {
            let fallback = text[range.clone()].to_owned();
            result.occurrences.push(EmojiOccurrence {
                kind: EmojiKind::Custom {
                    id: input.id.clone(),
                    format: input.format,
                    provenance: input.provenance,
                },
                fallback,
                utf8_range: range.clone(),
                utf16_range: input.utf16_range.clone(),
            });
            result.metrics.custom_count = result.metrics.custom_count.saturating_add(1);
            if matches!(
                input.format,
                CustomEmojiFormat::AnimatedTgs | CustomEmojiFormat::VideoWebm
            ) {
                result.metrics.animated_count = result.metrics.animated_count.saturating_add(1);
            }
            if input.format == CustomEmojiFormat::Unknown {
                result.metrics.unknown_format_count =
                    result.metrics.unknown_format_count.saturating_add(1);
            }
            add_occurrence(
                &mut counts,
                &mut run,
                &mut result.metrics,
                EmojiIdentity::Custom(input.id.0.clone()),
            );
            add_moderation_occurrence(
                &mut moderation_counts,
                &mut moderation_run,
                &mut result.metrics,
                EmojiIdentity::Custom(input.id.0.clone()),
            );
            utf16 = input.utf16_range.end;
            byte = range.end;
            custom_index += 1;
            continue;
        }

        let remaining = &text[byte..];
        let grapheme = graphemes(remaining)
            .next()
            .filter(|grapheme| !grapheme.is_empty())
            .ok_or(EmojiAnalysisError::InvalidGraphemeSegmentation)?;
        let byte_end = byte + grapheme.len();
        let utf16_end = utf16 + grapheme.encode_utf16().count();
        if grapheme.chars().all(char::is_whitespace) {
            run = 0;
            moderation_run = 0;
        } else if let Some(moderation_identity) = moderation_emoji(grapheme) {
            add_moderation_occurrence(
                &mut moderation_counts,
                &mut moderation_run,
                &mut result.metrics,
                EmojiIdentity::Unicode(moderation_identity.clone()),
            );
            if emojis::get(grapheme).is_some() {
                result.metrics.moderation_unicode_count =
                    result.metrics.moderation_unicode_count.saturating_add(1);
                let identity = EmojiIdentity::Unicode(canonical_emoji(grapheme));
                result.occurrences.push(EmojiOccurrence {
                    kind: EmojiKind::Unicode {
                        sequence: grapheme.to_owned(),
                    },
                    fallback: grapheme.to_owned(),
                    utf8_range: byte..byte_end,
                    utf16_range: utf16..utf16_end,
                });
                result.metrics.unicode_count = result.metrics.unicode_count.saturating_add(1);
                add_occurrence(&mut counts, &mut run, &mut result.metrics, identity);
            } else {
                result.metrics.moderation_unicode_count =
                    result.metrics.moderation_unicode_count.saturating_add(1);
                run = 0;
                result.metrics.visible_non_emoji_graphemes =
                    result.metrics.visible_non_emoji_graphemes.saturating_add(1);
            }
        } else {
            result.metrics.visible_non_emoji_graphemes =
                result.metrics.visible_non_emoji_graphemes.saturating_add(1);
            if !grapheme.chars().all(is_default_ignorable) {
                result.metrics.moderation_visible_non_emoji_graphemes = result
                    .metrics
                    .moderation_visible_non_emoji_graphemes
                    .saturating_add(1);
                run = 0;
                moderation_run = 0;
            }
        }
        byte = byte_end;
        utf16 = utf16_end;
    }

    result.metrics.unique_count = u32::try_from(counts.len()).unwrap_or(u32::MAX);
    result.metrics.moderation_unique_count =
        u32::try_from(moderation_counts.len()).unwrap_or(u32::MAX);
    let emoji = u64::from(result.metrics.unicode_count) + u64::from(result.metrics.custom_count);
    let visible = emoji + u64::from(result.metrics.visible_non_emoji_graphemes);
    result.metrics.density = (visible != 0).then(|| emoji as f64 / visible as f64);
    let moderation_emoji =
        u64::from(result.metrics.moderation_unicode_count) + u64::from(result.metrics.custom_count);
    let moderation_visible =
        moderation_emoji + u64::from(result.metrics.moderation_visible_non_emoji_graphemes);
    result.metrics.moderation_density =
        (moderation_visible != 0).then(|| moderation_emoji as f64 / moderation_visible as f64);
    Ok(result)
}

fn is_default_ignorable(character: char) -> bool {
    CodePointSetData::new::<DefaultIgnorableCodePoint>().contains(character)
}

/// Returns the moderation identity for a grapheme. Exact Unicode sequences
/// always win; only a failed exact lookup permits removing default-ignorables.
/// ZWJ, variation selectors, and tags get conservative fallback attempts, but
/// explicit text presentation (VS15) is never discarded.
fn moderation_emoji(grapheme: &str) -> Option<String> {
    if let Some(emoji) = emojis::get(grapheme) {
        return Some(emoji.as_str().to_owned());
    }
    if grapheme.contains('\u{FE0E}') {
        return None;
    }

    let without_nonstructural_ignorables: String = grapheme
        .chars()
        .filter(|character| {
            !is_default_ignorable(*character)
                || matches!(*character as u32, 0x200D | 0xFE00..=0xFE0F | 0xE0020..=0xE007F)
        })
        .collect();
    if let Some(emoji) = recognized_candidate(&without_nonstructural_ignorables) {
        return Some(emoji);
    }

    let without_edge_zwj = without_nonstructural_ignorables.trim_matches('\u{200D}');
    if let Some(emoji) = recognized_candidate(without_edge_zwj) {
        return Some(emoji);
    }

    let without_vs16: String = without_nonstructural_ignorables
        .chars()
        .filter(|character| *character != '\u{FE0F}')
        .collect();
    recognized_candidate(&without_vs16)
}

fn recognized_candidate(candidate: &str) -> Option<String> {
    (!candidate.is_empty())
        .then(|| emojis::get(candidate))
        .flatten()
        .map(|emoji| emoji.as_str().to_owned())
}

fn canonical_emoji(sequence: &str) -> String {
    emojis::get(sequence)
        .map(|emoji| emoji.as_str().to_owned())
        .unwrap_or_else(|| sequence.to_owned())
}

/// Adapts Telegram text/caption entities into the checked pure analyzer.
/// Custom emoji sticker format is `Unknown` until metadata lookup resolves it.
pub fn analyze_telegram_text(
    text: &str,
    entities: &[MessageEntity],
) -> Result<EmojiAnalysis, EmojiAnalysisError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(EmojiAnalysisError::TextTooLong);
    }
    let native_count = entities
        .iter()
        .filter(|entity| matches!(&entity.kind, MessageEntityKind::CustomEmoji { .. }))
        .count();
    let fallback_candidate_count = entities
        .iter()
        .filter(|entity| matches!(&entity.kind, MessageEntityKind::TextLink { .. }))
        .count();
    if native_count > MAX_CUSTOM_EMOJI_SPANS
        || native_count.saturating_add(fallback_candidate_count)
            > MAX_CUSTOM_EMOJI_SPANS.saturating_mul(2)
    {
        return Err(EmojiAnalysisError::TooManyCustomEmojiSpans);
    }

    let mut custom = Vec::with_capacity(native_count);
    let mut known_spans = HashMap::<(usize, usize, String), usize>::new();
    for entity in entities {
        if let MessageEntityKind::CustomEmoji { custom_emoji_id } = &entity.kind {
            if custom_emoji_id.0.len() > MAX_CUSTOM_EMOJI_ID_BYTES {
                return Err(EmojiAnalysisError::CustomEmojiIdTooLong);
            }
            let end = entity
                .offset
                .checked_add(entity.length)
                .ok_or(EmojiAnalysisError::InvalidUtf16Boundary)?;
            let index = custom.len();
            let id = custom_emoji_id.0.clone();
            known_spans.insert((entity.offset, end, id.clone()), index);
            custom.push(CustomEmojiInput {
                id: custom_emoji_id.clone(),
                utf16_range: entity.offset..end,
                format: CustomEmojiFormat::Unknown,
                provenance: CustomEmojiProvenance::Native,
            });
        }
    }
    let offsets = Utf16OffsetMap::new(text);
    for entity in entities {
        let MessageEntityKind::TextLink { url } = &entity.kind else {
            continue;
        };
        let Some(TelegramLink::CustomEmojiFallback { id }) =
            teloxide_url::parse_telegram_link(url.as_str())
        else {
            continue;
        };
        let Some(end) = entity.offset.checked_add(entity.length) else {
            continue;
        };
        let Some(span) = offsets.range(entity.offset, entity.length) else {
            continue;
        };
        let Some(label) = text.get(span.clone()) else {
            continue;
        };
        if !is_single_grapheme(label) || emojis::get(label).is_none() {
            continue;
        }
        let fallback = CustomEmojiInput {
            id: CustomEmojiId(id.to_string()),
            utf16_range: entity.offset..end,
            format: CustomEmojiFormat::Unknown,
            provenance: CustomEmojiProvenance::UrlFallback,
        };
        let key = (
            fallback.utf16_range.start,
            fallback.utf16_range.end,
            fallback.id.0.clone(),
        );
        if let Some(index) = known_spans.get(&key).copied() {
            let matched = &mut custom[index];
            matched.provenance = matched.provenance.merge(CustomEmojiProvenance::UrlFallback);
        } else {
            if custom.len() >= MAX_CUSTOM_EMOJI_SPANS {
                return Err(EmojiAnalysisError::TooManyCustomEmojiSpans);
            }
            known_spans.insert(key, custom.len());
            custom.push(fallback);
        }
    }
    analyze(text, &custom)
}

fn add_occurrence(
    counts: &mut HashMap<EmojiIdentity, u32>,
    run: &mut u32,
    metrics: &mut EmojiMetrics,
    identity: EmojiIdentity,
) {
    *run = run.saturating_add(1);
    metrics.max_consecutive_run = metrics.max_consecutive_run.max(*run);
    let count = counts.entry(identity).or_default();
    *count = count.saturating_add(1);
    metrics.max_repeat_count = metrics.max_repeat_count.max(*count);
}

fn add_moderation_occurrence(
    counts: &mut HashMap<EmojiIdentity, u32>,
    run: &mut u32,
    metrics: &mut EmojiMetrics,
    identity: EmojiIdentity,
) {
    *run = run.saturating_add(1);
    metrics.moderation_max_consecutive_run = metrics.moderation_max_consecutive_run.max(*run);
    let count = counts.entry(identity).or_default();
    *count = count.saturating_add(1);
    metrics.moderation_max_repeat_count = metrics.moderation_max_repeat_count.max(*count);
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    #[cfg(feature = "emoji-legacy-differential")]
    use unicode_segmentation::UnicodeSegmentation;

    #[test]
    fn grapheme_iterator_produces_strictly_advancing_ranges() {
        for text in ["", "ASCII", "👩🏽‍💻", "क्‍ष", "🏳️‍🌈"] {
            let mut expected_start = 0;
            for (start, grapheme) in grapheme_indices(text) {
                assert_eq!(start, expected_start, "grapheme gap in {text:?}");
                assert!(!grapheme.is_empty(), "empty grapheme in {text:?}");
                expected_start += grapheme.len();
            }
            assert_eq!(
                expected_start,
                text.len(),
                "incomplete segmentation of {text:?}"
            );
        }
    }

    #[test]
    fn icu_segmenter_matches_unicode_18_grapheme_break_test() {
        let mut cases = 0usize;
        for line in include_str!("../../test-data/unicode/18.0.0/GraphemeBreakTest.txt").lines() {
            let body = line.split('#').next().unwrap_or_default().trim();
            if body.is_empty() {
                continue;
            }
            let mut text = String::new();
            let mut expected = Vec::new();
            for token in body.split_whitespace() {
                match token {
                    "÷" => expected.push(text.len()),
                    "×" => {}
                    codepoint => {
                        let scalar = u32::from_str_radix(codepoint, 16).unwrap();
                        text.push(char::from_u32(scalar).expect("corpus contains scalar values"));
                    }
                }
            }

            let actual: Vec<_> = grapheme_indices(&text)
                .map(|(start, _)| start)
                .chain([text.len()])
                .collect();
            assert_eq!(actual, expected, "ICU segmentation for {body}");
            cases += 1;
        }
        eprintln!("ICU4X 2.4.0 neo conforms to {cases} Unicode 18 GraphemeBreakTest cases");
        assert!(cases > 800, "the official corpus should not be truncated");
    }

    #[cfg(feature = "emoji-legacy-differential")]
    #[test]
    fn unicode_segmentation_17_differential_is_an_explicit_unicode_18_change() {
        let mut cases = 0usize;
        let mut legacy_mismatches = 0usize;
        let mut legacy_difference_examples = Vec::new();
        for line in include_str!("../../test-data/unicode/18.0.0/GraphemeBreakTest.txt").lines() {
            let body = line.split('#').next().unwrap_or_default().trim();
            if body.is_empty() {
                continue;
            }
            let mut text = String::new();
            let mut expected = Vec::new();
            for token in body.split_whitespace() {
                match token {
                    "÷" => expected.push(text.len()),
                    "×" => {}
                    codepoint => text.push(
                        char::from_u32(u32::from_str_radix(codepoint, 16).unwrap())
                            .expect("corpus contains scalar values"),
                    ),
                }
            }
            let legacy: Vec<_> = text
                .grapheme_indices(true)
                .map(|(start, _)| start)
                .chain([text.len()])
                .collect();
            if legacy != expected {
                legacy_mismatches += 1;
                legacy_difference_examples.push(body.to_owned());
            }
            cases += 1;
        }
        eprintln!(
            "unicode-segmentation 1.13.3 differs from Unicode 18 in {legacy_mismatches}/{cases} cases: {legacy_difference_examples:?}"
        );
        assert_eq!(
            legacy_mismatches, 10,
            "review the pinned unicode-segmentation differential before changing its version"
        );
    }

    #[test]
    fn neo_segmenter_applies_indic_conjunct_rule_missing_from_v1() {
        let indic_conjunct = "\u{094d}\u{0915}";
        assert_eq!(
            GraphemeClusterSegmenter::new()
                .segment_str(indic_conjunct)
                .collect::<Vec<_>>(),
            [0, 3, 6]
        );
        assert_eq!(
            grapheme_indices(indic_conjunct)
                .map(|(start, _)| start)
                .chain([indic_conjunct.len()])
                .collect::<Vec<_>>(),
            [0, 6]
        );
    }

    #[test]
    fn pinned_emoji_recognizer_covers_unicode_18_fully_qualified_emoji_test() {
        let mut cases = 0usize;
        for line in include_str!("../../test-data/unicode/18.0.0/emoji-test.txt").lines() {
            let data = line.split('#').next().unwrap_or_default().trim();
            let Some((sequence, status)) = data.split_once(';') else {
                continue;
            };
            if status.trim() != "fully-qualified" {
                continue;
            }
            let sequence: String = sequence
                .split_whitespace()
                .map(|point| char::from_u32(u32::from_str_radix(point, 16).unwrap()).unwrap())
                .collect();
            assert!(
                emojis::get(&sequence).is_some(),
                "pinned `emojis` dataset does not recognize Unicode 18 sequence {sequence:?}"
            );
            cases += 1;
        }
        assert!(
            cases > 3000,
            "the official emoji corpus should not be truncated"
        );
    }

    #[test]
    fn pinned_emoji_recognizer_covers_unicode_18_sequence_corpora() {
        let mut sequence_cases = 0usize;
        let mut recognized = 0usize;
        let mut unsupported_examples = Vec::new();
        for path_data in [
            include_str!("../../test-data/unicode/18.0.0/emoji-sequences.txt"),
            include_str!("../../test-data/unicode/18.0.0/emoji-zwj-sequences.txt"),
        ] {
            for line in path_data.lines() {
                let data = line.split('#').next().unwrap_or_default().trim();
                let Some((codepoints, sequence_type)) = data.split_once(';') else {
                    continue;
                };
                if sequence_type.trim() == "Emoji_Modifier" {
                    continue;
                }
                let tokens: Vec<_> = codepoints.split_whitespace().collect();
                if tokens.len() == 1 && tokens[0].contains("..") {
                    let (start, end) = tokens[0].split_once("..").unwrap();
                    for point in u32::from_str_radix(start, 16).unwrap()
                        ..=u32::from_str_radix(end, 16).unwrap()
                    {
                        let sequence = char::from_u32(point).unwrap().to_string();
                        if emojis::get(&sequence).is_some() {
                            recognized += 1;
                        } else {
                            unsupported_examples.push(sequence);
                        }
                        sequence_cases += 1;
                    }
                    continue;
                }
                let sequence: String = tokens
                    .iter()
                    .map(|point| char::from_u32(u32::from_str_radix(point, 16).unwrap()).unwrap())
                    .collect();
                if emojis::get(&sequence).is_some() {
                    recognized += 1;
                } else {
                    unsupported_examples.push(sequence);
                }
                sequence_cases += 1;
            }
        }
        assert!(
            sequence_cases > 3900,
            "the official emoji sequence corpora should not be truncated"
        );
        eprintln!(
            "Unicode 18 emoji-sequences and emoji-zwj-sequences: {recognized}/{sequence_cases} sequence forms recognized directly; unsupported sample: {:?}",
            &unsupported_examples[..unsupported_examples.len().min(12)]
        );
        assert_eq!(sequence_cases, 3972, "review corpus version changes");
        assert_eq!(
            recognized, 3963,
            "review missing or newly-recognized emoji forms"
        );
        assert_eq!(
            unsupported_examples,
            ["🏻", "🏼", "🏽", "🏾", "🏿", "🦰", "🦱", "🦲", "🦳"],
            "only standalone modifier and hair components are intentionally absent"
        );
    }

    #[test]
    fn recognizes_flags_skin_tones_keycaps_and_zwj_sequences() {
        for sequence in ["🇺🇸", "👍🏽", "1️⃣", "👨‍👩‍👧‍👦"] {
            assert_eq!(
                analyze(sequence, &[]).unwrap().occurrences.len(),
                1,
                "expected one occurrence for {sequence}"
            );
        }
    }

    #[test]
    fn canonical_emoji_identity_groups_qualification_without_losing_input() {
        let unqualified = "\u{1f43f}";
        let canonical = emojis::get(unqualified).unwrap().as_str();
        assert_ne!(unqualified, canonical);

        let text = format!("{unqualified} {canonical}");
        let analysis = analyze(&text, &[]).unwrap();
        assert_eq!(analysis.occurrences.len(), 2);
        assert_eq!(analysis.occurrences[0].fallback, unqualified);
        assert_eq!(analysis.occurrences[1].fallback, canonical);
        assert_eq!(
            analysis.occurrences[0].kind.identity(),
            analysis.occurrences[1].kind.identity()
        );
        assert_eq!(analysis.metrics.unique_count, 1);
    }

    #[test]
    fn pinned_unicode_golden_fixture_matches_grapheme_and_emoji_counts() {
        #[derive(serde::Deserialize)]
        struct Fixture {
            cases: Vec<Case>,
        }

        #[derive(serde::Deserialize)]
        struct Case {
            text: String,
            graphemes: usize,
            emoji: usize,
        }

        let fixture: Fixture =
            serde_json::from_str(include_str!("../../test-data/unicode/cases.json"))
                .expect("unicode fixture is valid JSON");
        for case in fixture.cases {
            assert_eq!(
                graphemes(&case.text).count(),
                case.graphemes,
                "grapheme count for {:?}",
                case.text
            );
            assert_eq!(
                analyze(&case.text, &[]).unwrap().occurrences.len(),
                case.emoji,
                "emoji count for {:?}",
                case.text
            );
        }
    }

    #[test]
    fn density_runs_repeats_whitespace_and_visible_text_are_explicit() {
        let analysis = analyze("😀😀  A", &[]).unwrap();
        assert_eq!(analysis.metrics.unicode_count, 2);
        assert_eq!(analysis.metrics.visible_non_emoji_graphemes, 1);
        assert_eq!(analysis.metrics.max_consecutive_run, 2);
        assert_eq!(analysis.metrics.max_repeat_count, 2);
        assert_eq!(analysis.metrics.unique_count, 1);
        assert_eq!(analysis.metrics.density, Some(2.0 / 3.0));
        assert_eq!(analysis.metrics.moderation_density, Some(2.0 / 3.0));
        assert_eq!(analyze(" \n\t", &[]).unwrap().metrics.density, None);
        assert_eq!(analyze("", &[]).unwrap().metrics.density, None);
    }

    #[test]
    fn default_ignorables_are_raw_but_not_moderation_visible_or_run_breaks() {
        let analysis = analyze("😀\u{200B}😀\u{200B}A", &[]).unwrap();
        assert_eq!(analysis.metrics.unicode_count, 2);
        assert_eq!(analysis.metrics.visible_non_emoji_graphemes, 3);
        assert_eq!(analysis.metrics.moderation_visible_non_emoji_graphemes, 1);
        assert_eq!(analysis.metrics.max_consecutive_run, 2);
        assert_eq!(analysis.metrics.density, Some(2.0 / 5.0));
        assert_eq!(analysis.metrics.moderation_density, Some(2.0 / 3.0));

        // The family sequence remains a single recognized grapheme, including
        // its default-ignorable ZWJs and variation selectors.
        let family = analyze("👨‍👩‍👧‍👦", &[]).unwrap();
        assert_eq!(family.metrics.unicode_count, 1);
        assert_eq!(family.metrics.moderation_density, Some(1.0));
    }

    #[test]
    fn icu_default_ignorables_match_unicode_18_for_every_scalar_value() {
        let mut expected = vec![false; 0x11_0000];
        for line in include_str!("../../test-data/unicode/18.0.0/DerivedCoreProperties.txt").lines()
        {
            let data = line.split('#').next().unwrap_or_default().trim();
            let Some((range, property)) = data.split_once(';') else {
                continue;
            };
            if property.trim() != "Default_Ignorable_Code_Point" {
                continue;
            }
            let (start, end) = match range.trim().split_once("..") {
                Some((start, end)) => (
                    u32::from_str_radix(start, 16).unwrap(),
                    u32::from_str_radix(end, 16).unwrap(),
                ),
                None => {
                    let point = u32::from_str_radix(range.trim(), 16).unwrap();
                    (point, point)
                }
            };
            expected[start as usize..=end as usize].fill(true);
        }

        let actual = CodePointSetData::new::<DefaultIgnorableCodePoint>();
        let mismatches: Vec<_> = expected
            .iter()
            .enumerate()
            .filter_map(|(codepoint, expected)| {
                char::from_u32(codepoint as u32)
                    .filter(|character| actual.contains(*character) != *expected)
                    .map(|_| (codepoint, *expected))
            })
            .collect();
        assert!(
            mismatches.is_empty(),
            "ICU4X Default_Ignorable_Code_Point differs from Unicode 18.0 at {mismatches:?}"
        );
    }

    #[test]
    fn v5_moderation_hardening_fixture_keeps_raw_and_moderation_metrics_separate() {
        #[derive(serde::Deserialize)]
        struct Fixture {
            cases: Vec<Case>,
        }

        #[derive(serde::Deserialize)]
        struct Case {
            text: String,
            raw_exact_emoji: Option<usize>,
            moderation_emoji: u32,
            moderation_density: Option<f64>,
        }

        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../test-data/unicode/moderation-hardening.json"
        ))
        .expect("moderation hardening fixture is valid JSON");
        for case in fixture.cases {
            let analysis = analyze(&case.text, &[]).unwrap();
            if let Some(expected) = case.raw_exact_emoji {
                assert_eq!(analysis.occurrences.len(), expected, "{}", case.text);
            }
            assert_eq!(
                analysis.metrics.moderation_unicode_count, case.moderation_emoji,
                "{}",
                case.text
            );
            if let Some(expected) = case.moderation_density {
                assert_eq!(analysis.metrics.moderation_density, Some(expected));
            }
        }
    }

    #[test]
    fn custom_emoji_is_one_atomic_occurrence_and_does_not_double_count_fallback() {
        let analysis = analyze(
            "😀",
            &[CustomEmojiInput {
                id: CustomEmojiId("123".into()),
                utf16_range: 0..2,
                format: CustomEmojiFormat::Unknown,
                provenance: CustomEmojiProvenance::Native,
            }],
        )
        .unwrap();
        assert_eq!(analysis.metrics.unicode_count, 0);
        assert_eq!(analysis.metrics.custom_count, 1);
        assert_eq!(analysis.metrics.unknown_format_count, 1);
        assert_eq!(analysis.occurrences[0].utf8_range, 0..4);
        assert_eq!(analysis.occurrences[0].utf16_range, 0..2);
    }

    #[test]
    fn telegram_custom_entity_uses_utf16_range_and_unknown_format() {
        let entities = [MessageEntity::custom_emoji("123".into(), 0, 2)];
        let analysis = analyze_telegram_text("😀", &entities).unwrap();
        assert_eq!(analysis.metrics.unicode_count, 0);
        assert_eq!(analysis.metrics.custom_count, 1);
        assert_eq!(analysis.metrics.unknown_format_count, 1);
        assert_eq!(analysis.occurrences[0].utf8_range, 0..4);
        assert_eq!(
            analysis.occurrences[0].kind.encoding(),
            EmojiEncoding::NativeCustom
        );
    }

    #[test]
    fn addressed_custom_emoji_url_fallback_uses_id_and_validates_full_utf16_label() {
        let valid: MessageEntity = serde_json::from_value(serde_json::json!({
            "type":"text_link", "offset":1, "length":2, "url":"tg://emoji?id=00042"
        }))
        .unwrap();
        let analysis = analyze_telegram_text("A😀B", &[valid]).unwrap();
        assert_eq!(analysis.occurrences.len(), 1);
        assert_eq!(
            analysis.occurrences[0].kind.identity(),
            EmojiIdentity::Custom("42".into())
        );
        assert_eq!(analysis.occurrences[0].utf8_range, 1..5);
        assert_eq!(analysis.occurrences[0].utf16_range, 1..3);
        assert_eq!(
            analysis.occurrences[0].kind.encoding(),
            EmojiEncoding::CustomUrlFallback
        );
        assert_eq!(analysis.metrics.unicode_count, 0);

        let non_emoji: MessageEntity = serde_json::from_value(serde_json::json!({
            "type":"text_link", "offset":0, "length":5, "url":"tg://emoji?id=42"
        }))
        .unwrap();
        let invalid = analyze_telegram_text("hello", &[non_emoji]).unwrap();
        assert!(invalid.occurrences.is_empty());
        assert_eq!(invalid.metrics.custom_count, 0);
    }

    #[test]
    fn native_custom_and_same_id_fallback_deduplicate_preserving_provenance() {
        let native = MessageEntity::custom_emoji("42".into(), 0, 2);
        let fallback: MessageEntity = serde_json::from_value(serde_json::json!({
            "type":"text_link", "offset":0, "length":2, "url":"tg://emoji?id=42"
        }))
        .unwrap();
        let analysis = analyze_telegram_text("😀", &[native, fallback]).unwrap();
        assert_eq!(analysis.occurrences.len(), 1);
        assert_eq!(
            analysis.occurrences[0].kind.encoding(),
            EmojiEncoding::NativeCustomWithUrlFallback
        );
        assert!(
            analysis.occurrences[0]
                .kind
                .encoding()
                .has_custom_url_fallback()
        );
        assert_eq!(analysis.metrics.custom_count, 1);
    }

    #[test]
    fn repeated_url_fallbacks_do_not_fabricate_native_provenance() {
        let fallback: MessageEntity = serde_json::from_value(serde_json::json!({
            "type":"text_link", "offset":0, "length":2, "url":"tg://emoji?id=42"
        }))
        .unwrap();
        let analysis = analyze_telegram_text("😀", &[fallback.clone(), fallback]).unwrap();
        assert_eq!(analysis.occurrences.len(), 1);
        assert_eq!(
            analysis.occurrences[0].kind.encoding(),
            EmojiEncoding::CustomUrlFallback
        );
    }

    #[test]
    fn native_provenance_survives_repeated_matching_url_fallbacks() {
        let native = MessageEntity::custom_emoji("42".into(), 0, 2);
        let fallback: MessageEntity = serde_json::from_value(serde_json::json!({
            "type":"text_link", "offset":0, "length":2, "url":"tg://emoji?id=42"
        }))
        .unwrap();
        let analysis = analyze_telegram_text("😀", &[native, fallback.clone(), fallback]).unwrap();
        assert_eq!(analysis.occurrences.len(), 1);
        assert_eq!(
            analysis.occurrences[0].kind.encoding(),
            EmojiEncoding::NativeCustomWithUrlFallback
        );
    }

    #[test]
    fn provenance_union_is_idempotent_and_symmetric() {
        use CustomEmojiProvenance::{Native, NativeWithUrlFallback as Both, UrlFallback as Url};
        assert_eq!(Url.merge(Url), Url);
        assert_eq!(Native.merge(Url), Both);
        assert_eq!(Url.merge(Native), Both);
        assert_eq!(Both.merge(Url), Both);
    }

    #[test]
    fn conflicting_native_and_fallback_ids_on_same_span_fail_closed() {
        let native = MessageEntity::custom_emoji("42".into(), 0, 2);
        let fallback: MessageEntity = serde_json::from_value(serde_json::json!({
            "type":"text_link", "offset":0, "length":2, "url":"tg://emoji?id=43"
        }))
        .unwrap();
        assert_eq!(
            analyze_telegram_text("😀", &[native, fallback]),
            Err(EmojiAnalysisError::OverlappingCustomEmojiRanges)
        );
    }

    #[test]
    fn telegram_adapter_preflights_text_and_fallback_candidate_counts() {
        let too_long = "x".repeat(MAX_TEXT_BYTES + 1);
        assert_eq!(
            analyze_telegram_text(&too_long, &[]),
            Err(EmojiAnalysisError::TextTooLong)
        );
        let fallback: MessageEntity = serde_json::from_value(serde_json::json!({
            "type":"text_link", "offset":0, "length":2, "url":"tg://emoji?id=42"
        }))
        .unwrap();
        let too_many = vec![fallback; MAX_CUSTOM_EMOJI_SPANS * 2 + 1];
        assert_eq!(
            analyze_telegram_text("😀", &too_many),
            Err(EmojiAnalysisError::TooManyCustomEmojiSpans)
        );
        let long_id = [CustomEmojiInput {
            id: CustomEmojiId("x".repeat(MAX_CUSTOM_EMOJI_ID_BYTES + 1)),
            utf16_range: 0..2,
            format: CustomEmojiFormat::Unknown,
            provenance: CustomEmojiProvenance::Native,
        }];
        assert_eq!(
            analyze("😀", &long_id),
            Err(EmojiAnalysisError::CustomEmojiIdTooLong)
        );
    }

    #[test]
    fn malformed_and_overlapping_custom_spans_are_rejected() {
        let invalid = CustomEmojiInput {
            id: CustomEmojiId("1".into()),
            utf16_range: 1..2,
            format: CustomEmojiFormat::Static,
            provenance: CustomEmojiProvenance::Native,
        };
        assert_eq!(
            analyze("😀a", &[invalid]),
            Err(EmojiAnalysisError::InvalidUtf16Boundary)
        );
        let overlap = [
            CustomEmojiInput {
                id: CustomEmojiId("1".into()),
                utf16_range: 0..2,
                format: CustomEmojiFormat::Static,
                provenance: CustomEmojiProvenance::Native,
            },
            CustomEmojiInput {
                id: CustomEmojiId("2".into()),
                utf16_range: 1..3,
                format: CustomEmojiFormat::Static,
                provenance: CustomEmojiProvenance::Native,
            },
        ];
        assert_eq!(
            analyze("abc", &overlap),
            Err(EmojiAnalysisError::OverlappingCustomEmojiRanges)
        );

        let split_grapheme = [CustomEmojiInput {
            id: CustomEmojiId("3".into()),
            utf16_range: 0..1,
            format: CustomEmojiFormat::Static,
            provenance: CustomEmojiProvenance::Native,
        }];
        assert_eq!(
            analyze("a\u{301}", &split_grapheme),
            Err(EmojiAnalysisError::InvalidGraphemeBoundary)
        );
    }

    #[test]
    fn animated_and_unknown_custom_formats_are_counted_separately() {
        let analysis = analyze(
            "xy",
            &[
                CustomEmojiInput {
                    id: CustomEmojiId("animated".into()),
                    utf16_range: 0..1,
                    format: CustomEmojiFormat::AnimatedTgs,
                    provenance: CustomEmojiProvenance::Native,
                },
                CustomEmojiInput {
                    id: CustomEmojiId("unknown".into()),
                    utf16_range: 1..2,
                    format: CustomEmojiFormat::Unknown,
                    provenance: CustomEmojiProvenance::Native,
                },
            ],
        )
        .unwrap();
        assert_eq!(analysis.metrics.custom_count, 2);
        assert_eq!(analysis.metrics.animated_count, 1);
        assert_eq!(analysis.metrics.unknown_format_count, 1);
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(2048))]
        #[test]
        fn arbitrary_unicode_text_never_panics(text in ".*") {
            let analysis = analyze(&text, &[]).unwrap();
            for occurrence in analysis.occurrences {
                prop_assert!(text.is_char_boundary(occurrence.utf8_range.start));
                prop_assert!(text.is_char_boundary(occurrence.utf8_range.end));
                prop_assert!(occurrence.utf16_range.start < occurrence.utf16_range.end);
            }
        }

        #[test]
        fn generated_valid_custom_ranges_match_independent_utf16_oracle(
            chars in proptest::collection::vec(any::<char>(), 1..32),
            selected in 0usize..64,
        ) {
            let text: String = chars.into_iter().collect();
            let graphemes: Vec<_> = grapheme_indices(&text).collect();
            let index = selected % graphemes.len();
            let (byte_start, grapheme) = graphemes[index];
            let byte_end = byte_start + grapheme.len();
            let utf16_start = text[..byte_start].encode_utf16().count();
            let utf16_end = utf16_start + grapheme.encode_utf16().count();
            let custom = [CustomEmojiInput {
                id: CustomEmojiId("property-id".into()),
                utf16_range: utf16_start..utf16_end,
                format: CustomEmojiFormat::Static,
                provenance: CustomEmojiProvenance::Native,
                }];

            let analysis = analyze(&text, &custom).unwrap();
            let occurrence = analysis.occurrences.iter().find(|occurrence| {
                occurrence.kind.identity() == EmojiIdentity::Custom("property-id".into())
            }).unwrap();
            prop_assert_eq!(&occurrence.utf8_range, &(byte_start..byte_end));
            prop_assert_eq!(&occurrence.utf16_range, &(utf16_start..utf16_end));
            prop_assert_eq!(&occurrence.fallback, grapheme);
            prop_assert_eq!(analysis.metrics.custom_count, 1);
        }

        #[test]
        fn generated_custom_ranges_match_scalar_and_grapheme_boundary_oracle(
            chars in proptest::collection::vec(any::<char>(), 0..24),
            offset in 0usize..80,
            length in 0usize..24,
        ) {
            let text: String = chars.into_iter().collect();
            let end = offset + length;
            let input = [CustomEmojiInput {
                id: CustomEmojiId("oracle".into()),
                utf16_range: offset..end,
                format: CustomEmojiFormat::Static,
                provenance: CustomEmojiProvenance::Native,
                }];
            let oracle_boundaries: Vec<_> = text.char_indices()
                .map(|(byte, _)| (text[..byte].encode_utf16().count(), byte))
                .chain(std::iter::once((text.encode_utf16().count(), text.len())))
                .collect();
            let start_byte = oracle_boundaries.iter().find(|(units, _)| *units == offset).map(|(_, byte)| *byte);
            let end_byte = oracle_boundaries.iter().find(|(units, _)| *units == end).map(|(_, byte)| *byte);
            let grapheme_boundaries: HashSet<_> = grapheme_indices(&text)
                .map(|(byte, _)| byte)
                .chain(std::iter::once(text.len()))
                .collect();

            let expected_error = if offset >= end {
                Some(EmojiAnalysisError::EmptyCustomEmojiRange)
            } else if start_byte.is_none() || end_byte.is_none() {
                Some(EmojiAnalysisError::InvalidUtf16Boundary)
            } else if !grapheme_boundaries.contains(&start_byte.unwrap())
                || !grapheme_boundaries.contains(&end_byte.unwrap())
            {
                Some(EmojiAnalysisError::InvalidGraphemeBoundary)
            } else {
                None
            };
            let actual = analyze(&text, &input);
            if let Some(expected) = expected_error {
                prop_assert_eq!(actual, Err(expected));
            } else {
                let result = actual.unwrap();
                let occurrence = result.occurrences.iter().find(|item|
                    item.kind.identity() == EmojiIdentity::Custom("oracle".into())
                ).unwrap();
                prop_assert_eq!(&occurrence.utf8_range, &(start_byte.unwrap()..end_byte.unwrap()));
                prop_assert_eq!(&occurrence.utf16_range, &(offset..end));
            }
        }

        #[test]
        fn generated_cgj_contamination_preserves_moderation_emoji_but_not_raw_match(
            emoji in proptest::sample::select(vec!["😀", "🔥", "🚀"]),
            cgj_count in 1usize..4,
        ) {
            let contaminated = format!("{}{}", emoji, "\u{034F}".repeat(cgj_count));
            let analysis = analyze(&contaminated, &[]).unwrap();
            prop_assert!(analysis.occurrences.is_empty());
            prop_assert_eq!(analysis.metrics.unicode_count, 0);
            prop_assert_eq!(analysis.metrics.moderation_unicode_count, 1);
            prop_assert_eq!(analysis.metrics.moderation_density, Some(1.0));
        }
    }
}
