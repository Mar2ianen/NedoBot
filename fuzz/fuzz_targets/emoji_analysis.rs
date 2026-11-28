#![no_main]

use libfuzzer_sys::fuzz_target;
use teloxide::types::{
    CustomEmojiId, MessageEntity, MessageEntityKind, RichMessage,
};
use tg_ai_bot_teloxide::features::emoji_analysis::{
    CustomEmojiFormat, CustomEmojiInput, CustomEmojiProvenance, EmojiAnalysisError, EmojiKind,
    analyze, analyze_telegram_text, MAX_TEXT_BYTES,
};
use tg_ai_bot_teloxide::features::emoji_occurrences::extract_rich_message;

fn assert_offsets(text: &str, occurrences: &[impl OccurrenceRanges]) {
    for item in occurrences {
        let utf8 = item.utf8_range();
        assert!(utf8.start <= utf8.end && utf8.end <= text.len());
        assert!(text.is_char_boundary(utf8.start));
        assert!(text.is_char_boundary(utf8.end));
        assert_eq!(&text[utf8.clone()], item.fallback());
        assert_eq!(
            text[..utf8.start].encode_utf16().count(),
            item.utf16_range().start
        );
        assert_eq!(
            text[..utf8.end].encode_utf16().count(),
            item.utf16_range().end
        );
    }
}

trait OccurrenceRanges {
    fn utf8_range(&self) -> std::ops::Range<usize>;
    fn utf16_range(&self) -> std::ops::Range<usize>;
    fn fallback(&self) -> &str;
}

impl OccurrenceRanges for tg_ai_bot_teloxide::features::emoji_analysis::EmojiOccurrence {
    fn utf8_range(&self) -> std::ops::Range<usize> { self.utf8_range.clone() }
    fn utf16_range(&self) -> std::ops::Range<usize> { self.utf16_range.clone() }
    fn fallback(&self) -> &str { &self.fallback }
}

fn id(data: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut value = String::new();
    for byte in data.iter().take(24) {
        write!(&mut value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    if value.is_empty() { "0".to_owned() } else { value }
}

fuzz_target!(|data: &[u8]| {
    let split = data.len().min(MAX_TEXT_BYTES / 2);
    let text = String::from_utf8_lossy(&data[..split]).into_owned();
    let suffix = &data[split..];
    let emoji_id = id(suffix);

    // Reuse analyzer-confirmed Unicode grapheme ranges to generate partial,
    // adjacent, and intersecting custom entity spans without assuming an
    // independent segmenter's Unicode version.
    if !text.is_empty() {
        let end = text.encode_utf16().count();
        let source = analyze(&text, &[]).expect("bounded Unicode-only input must analyze");
        let mut custom: Vec<_> = source
            .occurrences
            .into_iter()
            .filter(|item| matches!(item.kind, EmojiKind::Unicode { .. }))
            .take(16)
            .enumerate()
            .map(|(index, item)| CustomEmojiInput {
                id: CustomEmojiId(format!("{emoji_id}-{index}")),
                utf16_range: item.utf16_range,
                format: if suffix.first().is_some_and(|byte| byte & 1 == 0) {
                    CustomEmojiFormat::AnimatedTgs
                } else {
                    CustomEmojiFormat::VideoWebm
                },
                provenance: if suffix.first().is_some_and(|byte| byte & 2 == 0) {
                    CustomEmojiProvenance::Native
                } else {
                    CustomEmojiProvenance::UrlFallback
                },
            })
            .collect();
        let has_emoji_span = !custom.is_empty();
        if custom.is_empty() {
            custom.push(CustomEmojiInput {
                id: CustomEmojiId(emoji_id.clone()),
                utf16_range: 0..end,
                format: CustomEmojiFormat::Unknown,
                provenance: CustomEmojiProvenance::Native,
            });
        }
        match analyze(&text, &custom) {
            Ok(analysis) => {
                assert_offsets(&text, &analysis.occurrences);
                assert_eq!(analysis.metrics.custom_count as usize, custom.len());
            }
            Err(error) => panic!("valid whole-text custom range rejected: {error:?}"),
        }

        let mut overlapping = custom.clone();
        overlapping.push(custom[0].clone());
        assert!(matches!(
            analyze(&text, &overlapping),
            Err(EmojiAnalysisError::OverlappingCustomEmojiRanges)
        ));

        let mut invalid_boundary = custom[0].clone();
        if has_emoji_span && invalid_boundary.utf16_range.end - invalid_boundary.utf16_range.start > 1 {
            invalid_boundary.utf16_range.start += 1;
            assert!(matches!(
                analyze(&text, &[invalid_boundary]),
                Err(EmojiAnalysisError::InvalidUtf16Boundary
                    | EmojiAnalysisError::InvalidGraphemeBoundary)
            ));
        }
    }

    // Exercise TextLink parsing and native + URL-fallback provenance through
    // the actual entity adapter, with valid UTF-16 ranges and URL spellings.
    let linked_text = format!("{text}😀");
    let offset = text.encode_utf16().count();
    let link = MessageEntity {
        kind: MessageEntityKind::TextLink {
            url: reqwest::Url::parse(&format!("tg://emoji?id={emoji_id}")).unwrap(),
        },
        offset,
        length: 2,
    };
    let native = MessageEntity {
        kind: MessageEntityKind::CustomEmoji {
            custom_emoji_id: CustomEmojiId(emoji_id.clone()),
        },
        offset,
        length: 2,
    };
    let linked = analyze_telegram_text(&linked_text, &[native, link]);
    let linked = linked.expect("bounded valid native and TextLink entities must analyze");
    assert_offsets(&linked_text, &linked.occurrences);
    assert!(linked.occurrences.iter().any(|item| item.fallback == "😀"));

    // Build a real RichMessage carrying style-split ZWJ/VS16 sequences,
    // ordinary fuzz text, nested custom emoji, and a Telegram URL fallback.
    let rich_text = serde_json::json!({"blocks":[{"type":"paragraph","text":[
        {"type":"bold","text":"👩‍"},
        {"type":"italic","text":"💻"},
        text,
        {"type":"strikethrough","text":"❤"},
        {"type":"code","text":"️"},
        {"type":"custom_emoji","custom_emoji_id":emoji_id,"alternative_text":"😀"},
        {"type":"url","url":format!("tg://emoji?id={emoji_id}"),"text":"😀"}
    ]}]});
    if let Ok(rich) = serde_json::from_value::<RichMessage>(rich_text) {
        match extract_rich_message(&rich) {
            Ok(occurrences) => {
                assert!(occurrences.iter().any(|item| item.fallback == "👩‍💻"));
                assert!(occurrences.iter().any(|item| item.fallback == "❤️"));
                for item in &occurrences {
                    assert_eq!(
                        item.fallback.encode_utf16().count(),
                        item.utf16_range.end - item.utf16_range.start
                    );
                    if !item.source_ranges.is_empty() {
                        assert_eq!(
                            item.source_ranges
                                .iter()
                                .map(|range| range.utf8_range.end - range.utf8_range.start)
                                .sum::<usize>(),
                            item.fallback.len()
                        );
                        assert_eq!(
                            item.source_ranges
                                .iter()
                                .map(|range| range.utf16_range.end - range.utf16_range.start)
                                .sum::<usize>(),
                            item.fallback.encode_utf16().count()
                        );
                    }
                }
            }
            Err(EmojiAnalysisError::RichTraversalLimitExceeded) => {
                assert!(text.len() > 4_096, "small structured RichMessage rejected")
            }
            Err(error) => panic!("unexpected structured RichMessage error: {error:?}"),
        }
    }
});
