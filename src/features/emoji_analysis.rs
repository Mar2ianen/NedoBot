//! Optional, deterministic Unicode/custom emoji extraction and metrics.
//!
//! Recognition uses the exact Unicode data pinned by `emojis`; this module
//! does not fetch metadata, decode media, or apply moderation policy.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use teloxide::types::{MessageEntity, MessageEntityKind, Utf16OffsetMap};
use unicode_segmentation::UnicodeSegmentation;

/// Unicode emoji data version provided by the pinned recognition dataset.
pub const UNICODE_EMOJI_VERSION: &str = "18.0";

pub const MAX_TEXT_BYTES: usize = 1_048_576;
pub const MAX_CUSTOM_EMOJI_SPANS: usize = 2_048;

// Unicode 18.0.0 DerivedCoreProperties.txt, Default_Ignorable_Code_Point.
// Keep this aligned with UNICODE_EMOJI_VERSION; ZWJ sequences are checked as
// emoji graphemes before this property is used for moderation visibility.
const DEFAULT_IGNORABLE_RANGES: &[(u32, u32)] = &[
    (0x00AD, 0x00AD),
    (0x034F, 0x034F),
    (0x061C, 0x061C),
    (0x115F, 0x1160),
    (0x17B4, 0x17B5),
    (0x180B, 0x180F),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x206F),
    (0x3164, 0x3164),
    (0xFE00, 0xFE0F),
    (0xFEFF, 0xFEFF),
    (0xFFA0, 0xFFA0),
    (0xFFF0, 0xFFF8),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0000, 0xE0FFF),
];

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

/// A Telegram custom emoji's UTF-16 range and optional known representation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustomEmojiInput {
    pub id: String,
    pub utf16_range: Range<usize>,
    pub format: EmojiFormat,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmojiOccurrence {
    pub identity: EmojiIdentity,
    pub fallback: String,
    pub utf8_range: Range<usize>,
    pub utf16_range: Range<usize>,
    pub format: EmojiFormat,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmojiMetrics {
    pub unicode_count: u32,
    pub custom_count: u32,
    pub animated_count: u32,
    pub unknown_format_count: u32,
    /// Non-emoji graphemes before default-ignorable filtering.
    pub visible_non_emoji_graphemes: u32,
    /// Non-emoji graphemes visible to moderation after default-ignorable filtering.
    pub moderation_visible_non_emoji_graphemes: u32,
    pub max_consecutive_run: u32,
    pub max_repeat_count: u32,
    pub unique_count: u32,
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
    EmptyCustomEmojiId,
    EmptyCustomEmojiRange,
    InvalidUtf16Boundary,
    InvalidGraphemeBoundary,
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

    let boundaries = Utf16OffsetMap::new(text);
    let grapheme_boundaries: HashSet<_> = text
        .grapheme_indices(true)
        .map(|(byte, _)| byte)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut custom_spans = custom
        .iter()
        .map(|item| {
            if item.id.is_empty() {
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
            let byte_start = byte_range.start;
            let byte_end = byte_range.end;
            if !grapheme_boundaries.contains(&byte_start)
                || !grapheme_boundaries.contains(&byte_end)
            {
                return Err(EmojiAnalysisError::InvalidGraphemeBoundary);
            }
            Ok((byte_start..byte_end, item))
        })
        .collect::<Result<Vec<_>, _>>()?;
    custom_spans.sort_by_key(|(bytes, _)| bytes.start);
    if custom_spans
        .windows(2)
        .any(|spans| spans[0].0.end > spans[1].0.start)
    {
        return Err(EmojiAnalysisError::OverlappingCustomEmojiRanges);
    }

    let mut result = EmojiAnalysis {
        occurrences: Vec::new(),
        metrics: EmojiMetrics::default(),
    };
    let mut counts = HashMap::<EmojiIdentity, u32>::new();
    let mut run = 0u32;
    let mut byte = 0usize;
    let mut utf16 = 0usize;
    let mut custom_index = 0usize;

    while byte < text.len() {
        if let Some((range, input)) = custom_spans.get(custom_index)
            && range.start == byte
        {
            let fallback = text[range.clone()].to_owned();
            result.occurrences.push(EmojiOccurrence {
                identity: EmojiIdentity::Custom(input.id.clone()),
                fallback,
                utf8_range: range.clone(),
                utf16_range: input.utf16_range.clone(),
                format: input.format,
            });
            result.metrics.custom_count = result.metrics.custom_count.saturating_add(1);
            if matches!(
                input.format,
                EmojiFormat::AnimatedTgs | EmojiFormat::VideoWebm
            ) {
                result.metrics.animated_count = result.metrics.animated_count.saturating_add(1);
            }
            if input.format == EmojiFormat::Unknown {
                result.metrics.unknown_format_count =
                    result.metrics.unknown_format_count.saturating_add(1);
            }
            add_occurrence(
                &mut counts,
                &mut run,
                &mut result.metrics,
                EmojiIdentity::Custom(input.id.clone()),
            );
            utf16 = input.utf16_range.end;
            byte = range.end;
            custom_index += 1;
            continue;
        }

        let remaining = &text[byte..];
        let grapheme = remaining
            .graphemes(true)
            .next()
            .expect("non-empty remainder has a grapheme");
        let byte_end = byte + grapheme.len();
        let utf16_end = utf16 + grapheme.encode_utf16().count();
        if grapheme.chars().all(char::is_whitespace) {
            run = 0;
        } else if emojis::get(grapheme).is_some() {
            let identity = EmojiIdentity::Unicode(grapheme.to_owned());
            result.occurrences.push(EmojiOccurrence {
                identity: identity.clone(),
                fallback: grapheme.to_owned(),
                utf8_range: byte..byte_end,
                utf16_range: utf16..utf16_end,
                format: EmojiFormat::Unicode,
            });
            result.metrics.unicode_count = result.metrics.unicode_count.saturating_add(1);
            add_occurrence(&mut counts, &mut run, &mut result.metrics, identity);
        } else {
            result.metrics.visible_non_emoji_graphemes =
                result.metrics.visible_non_emoji_graphemes.saturating_add(1);
            if !grapheme.chars().all(is_default_ignorable) {
                result.metrics.moderation_visible_non_emoji_graphemes = result
                    .metrics
                    .moderation_visible_non_emoji_graphemes
                    .saturating_add(1);
                run = 0;
            }
        }
        byte = byte_end;
        utf16 = utf16_end;
    }

    result.metrics.unique_count = u32::try_from(counts.len()).unwrap_or(u32::MAX);
    let emoji = u64::from(result.metrics.unicode_count) + u64::from(result.metrics.custom_count);
    let visible = emoji + u64::from(result.metrics.visible_non_emoji_graphemes);
    result.metrics.density = (visible != 0).then(|| emoji as f64 / visible as f64);
    let moderation_visible =
        emoji + u64::from(result.metrics.moderation_visible_non_emoji_graphemes);
    result.metrics.moderation_density =
        (moderation_visible != 0).then(|| emoji as f64 / moderation_visible as f64);
    Ok(result)
}

fn is_default_ignorable(character: char) -> bool {
    let codepoint = character as u32;
    DEFAULT_IGNORABLE_RANGES
        .iter()
        .any(|&(start, end)| (start..=end).contains(&codepoint))
}

/// Adapts Telegram text/caption entities into the checked pure analyzer.
/// Custom emoji sticker format is `Unknown` until metadata lookup resolves it.
pub fn analyze_telegram_text(
    text: &str,
    entities: &[MessageEntity],
) -> Result<EmojiAnalysis, EmojiAnalysisError> {
    let mut custom = Vec::new();
    for entity in entities {
        if let MessageEntityKind::CustomEmoji { custom_emoji_id } = &entity.kind {
            let end = entity
                .offset
                .checked_add(entity.length)
                .ok_or(EmojiAnalysisError::InvalidUtf16Boundary)?;
            custom.push(CustomEmojiInput {
                id: custom_emoji_id.to_string(),
                utf16_range: entity.offset..end,
                format: EmojiFormat::Unknown,
            });
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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
                case.text.graphemes(true).count(),
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
    fn custom_emoji_is_one_atomic_occurrence_and_does_not_double_count_fallback() {
        let analysis = analyze(
            "😀",
            &[CustomEmojiInput {
                id: "123".to_owned(),
                utf16_range: 0..2,
                format: EmojiFormat::Unknown,
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
    }

    #[test]
    fn malformed_and_overlapping_custom_spans_are_rejected() {
        let invalid = CustomEmojiInput {
            id: "1".to_owned(),
            utf16_range: 1..2,
            format: EmojiFormat::Static,
        };
        assert_eq!(
            analyze("😀a", &[invalid]),
            Err(EmojiAnalysisError::InvalidUtf16Boundary)
        );
        let overlap = [
            CustomEmojiInput {
                id: "1".to_owned(),
                utf16_range: 0..2,
                format: EmojiFormat::Static,
            },
            CustomEmojiInput {
                id: "2".to_owned(),
                utf16_range: 1..3,
                format: EmojiFormat::Static,
            },
        ];
        assert_eq!(
            analyze("abc", &overlap),
            Err(EmojiAnalysisError::OverlappingCustomEmojiRanges)
        );

        let split_grapheme = [CustomEmojiInput {
            id: "3".to_owned(),
            utf16_range: 0..1,
            format: EmojiFormat::Static,
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
                    id: "animated".to_owned(),
                    utf16_range: 0..1,
                    format: EmojiFormat::AnimatedTgs,
                },
                CustomEmojiInput {
                    id: "unknown".to_owned(),
                    utf16_range: 1..2,
                    format: EmojiFormat::Unknown,
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
            let graphemes: Vec<_> = text.grapheme_indices(true).collect();
            let index = selected % graphemes.len();
            let (byte_start, grapheme) = graphemes[index];
            let byte_end = byte_start + grapheme.len();
            let utf16_start = text[..byte_start].encode_utf16().count();
            let utf16_end = utf16_start + grapheme.encode_utf16().count();
            let custom = [CustomEmojiInput {
                id: "property-id".into(),
                utf16_range: utf16_start..utf16_end,
                format: EmojiFormat::Static,
            }];

            let analysis = analyze(&text, &custom).unwrap();
            let occurrence = analysis.occurrences.iter().find(|occurrence| {
                occurrence.identity == EmojiIdentity::Custom("property-id".into())
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
                id: "oracle".into(),
                utf16_range: offset..end,
                format: EmojiFormat::Static,
            }];
            let oracle_boundaries: Vec<_> = text.char_indices()
                .map(|(byte, _)| (text[..byte].encode_utf16().count(), byte))
                .chain(std::iter::once((text.encode_utf16().count(), text.len())))
                .collect();
            let start_byte = oracle_boundaries.iter().find(|(units, _)| *units == offset).map(|(_, byte)| *byte);
            let end_byte = oracle_boundaries.iter().find(|(units, _)| *units == end).map(|(_, byte)| *byte);
            let grapheme_boundaries: HashSet<_> = text.grapheme_indices(true)
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
                    item.identity == EmojiIdentity::Custom("oracle".into())
                ).unwrap();
                prop_assert_eq!(&occurrence.utf8_range, &(start_byte.unwrap()..end_byte.unwrap()));
                prop_assert_eq!(&occurrence.utf16_range, &(offset..end));
            }
        }
    }
}
