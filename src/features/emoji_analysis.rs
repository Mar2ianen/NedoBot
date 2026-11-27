//! Optional, deterministic Unicode/custom emoji extraction and metrics.
//!
//! Recognition uses the exact Unicode data pinned by `emojis`; this module
//! does not fetch metadata, decode media, or apply moderation policy.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use teloxide::types::{MessageEntity, MessageEntityKind};
use unicode_segmentation::UnicodeSegmentation;

/// Unicode emoji data version provided by the pinned recognition dataset.
pub const UNICODE_EMOJI_VERSION: &str = "18.0";

pub const MAX_TEXT_BYTES: usize = 1_048_576;
pub const MAX_CUSTOM_EMOJI_SPANS: usize = 2_048;

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
    pub visible_non_emoji_graphemes: u32,
    pub max_consecutive_run: u32,
    pub max_repeat_count: u32,
    pub unique_count: u32,
    pub density: Option<f64>,
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

    let boundaries = Utf16Boundaries::new(text);
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
            let byte_start = boundaries
                .byte_offset(item.utf16_range.start)
                .ok_or(EmojiAnalysisError::InvalidUtf16Boundary)?;
            let byte_end = boundaries
                .byte_offset(item.utf16_range.end)
                .ok_or(EmojiAnalysisError::InvalidUtf16Boundary)?;
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
            run = 0;
        }
        byte = byte_end;
        utf16 = utf16_end;
    }

    result.metrics.unique_count = u32::try_from(counts.len()).unwrap_or(u32::MAX);
    let emoji = u64::from(result.metrics.unicode_count) + u64::from(result.metrics.custom_count);
    let visible = emoji + u64::from(result.metrics.visible_non_emoji_graphemes);
    result.metrics.density = (visible != 0).then(|| emoji as f64 / visible as f64);
    Ok(result)
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

struct Utf16Boundaries(Vec<(usize, usize)>);

impl Utf16Boundaries {
    fn new(text: &str) -> Self {
        let mut boundaries = Vec::with_capacity(text.chars().count().saturating_add(1));
        let (mut utf16, mut utf8) = (0usize, 0usize);
        for character in text.chars() {
            boundaries.push((utf16, utf8));
            utf16 += character.len_utf16();
            utf8 += character.len_utf8();
        }
        boundaries.push((utf16, utf8));
        Self(boundaries)
    }

    fn byte_offset(&self, target: usize) -> Option<usize> {
        self.0
            .binary_search_by_key(&target, |&(utf16, _)| utf16)
            .ok()
            .map(|index| self.0[index].1)
    }
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
        assert_eq!(analyze(" \n\t", &[]).unwrap().metrics.density, None);
        assert_eq!(analyze("", &[]).unwrap().metrics.density, None);
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
    }
}
