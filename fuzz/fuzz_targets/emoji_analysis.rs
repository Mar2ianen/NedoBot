#![no_main]

use libfuzzer_sys::fuzz_target;
use teloxide::types::{
    CustomEmojiId, MessageEntity, MessageEntityKind, RichMessage,
};
use tg_ai_bot_teloxide::features::emoji_analysis::{
    CustomEmojiFormat, CustomEmojiInput, CustomEmojiProvenance, EmojiAnalysisError, EmojiIdentity,
    EmojiKind, analyze, analyze_telegram_text, MAX_TEXT_BYTES,
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
    let value = data
        .iter()
        .fold(1_u64, |value, byte| value.wrapping_mul(257).wrapping_add(u64::from(*byte)))
        .max(1);
    value.to_string()
}

fuzz_target!(|data: &[u8]| {
    let split = (data.len() / 2).min(MAX_TEXT_BYTES / 2);
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

    // A control character forces a grapheme boundary even if arbitrary input
    // ends in Prepend/Extend/ZWJ. The id is always a positive decimal u64,
    // which is the URL parser's accepted wire format.
    let linked_text = format!("{text}\0😀");
    let offset = text.encode_utf16().count() + 1;
    let link = MessageEntity {
        kind: MessageEntityKind::TextLink {
            url: reqwest::Url::parse(&format!("tg://emoji?id={emoji_id}")).unwrap(),
        },
        offset,
        length: 2,
    };
    let fallback_only = analyze_telegram_text(&linked_text, &[link.clone()])
        .expect("valid TextLink fallback must analyze");
    assert_offsets(&linked_text, &fallback_only.occurrences);
    let fallback_occurrences: Vec<_> = fallback_only
        .occurrences
        .iter()
        .filter(|item| item.kind.identity() == EmojiIdentity::Custom(emoji_id.clone()))
        .collect();
    assert_eq!(fallback_occurrences.len(), 1);
    assert_eq!(fallback_occurrences[0].fallback, "😀");
    assert_eq!(
        fallback_occurrences[0].kind.encoding(),
        tg_ai_bot_teloxide::features::emoji_analysis::EmojiEncoding::CustomUrlFallback
    );
    assert!(matches!(
        fallback_occurrences[0].kind,
        EmojiKind::Custom {
            provenance: CustomEmojiProvenance::UrlFallback,
            ..
        }
    ));

    // Also check that a native entity at the same range merges provenance
    // rather than replacing the custom identity or counting twice.
    let native = MessageEntity {
        kind: MessageEntityKind::CustomEmoji {
            custom_emoji_id: CustomEmojiId(emoji_id.clone()),
        },
        offset,
        length: 2,
    };
    let linked = analyze_telegram_text(&linked_text, &[native, link])
        .expect("bounded valid native and TextLink entities must analyze");
    assert_offsets(&linked_text, &linked.occurrences);
    let merged: Vec<_> = linked
        .occurrences
        .iter()
        .filter(|item| item.kind.identity() == EmojiIdentity::Custom(emoji_id.clone()))
        .collect();
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].fallback, "😀");
    assert_eq!(
        merged[0].kind.encoding(),
        tg_ai_bot_teloxide::features::emoji_analysis::EmojiEncoding::NativeCustomWithUrlFallback
    );
    assert!(matches!(
        merged[0].kind,
        EmojiKind::Custom {
            provenance: CustomEmojiProvenance::NativeWithUrlFallback,
            ..
        }
    ));

    // Build a real RichMessage carrying style-split ZWJ/VS16 sequences,
    // ordinary fuzz text, nested custom emoji, and a Telegram URL fallback.
    // Keep this fragment bounded so every generated object should stay inside
    // RichMessage traversal budgets, making all errors actionable.
    let rich_fragment: String = text.chars().take(1_024).collect();
    let rich_text = serde_json::json!({"blocks":[{"type":"paragraph","text":[
        {"type":"bold","text":"👩‍"},
        {"type":"italic","text":"💻"},
        "\0",
        rich_fragment,
        "\0",
        {"type":"strikethrough","text":"❤"},
        {"type":"code","text":"️"},
        "\0",
        {"type":"custom_emoji","custom_emoji_id":emoji_id,"alternative_text":"😀"},
        {"type":"url","url":format!("tg://emoji?id={emoji_id}"),"text":"😀"}
    ]}]});
    let rich = serde_json::from_value::<RichMessage>(rich_text)
        .expect("structured RichMessage fixture must deserialize");
    let occurrences = extract_rich_message(&rich)
        .expect("bounded generated RichMessage must pass traversal limits");
    assert!(occurrences.iter().any(|item| item.fallback == "👩‍💻"));
    assert!(occurrences.iter().any(|item| item.fallback == "❤️"));
    assert!(occurrences.iter().any(|item| {
        item.kind.identity() == EmojiIdentity::Custom(emoji_id.clone())
            && item.fallback == "😀"
            && item.kind.encoding()
                == tg_ai_bot_teloxide::features::emoji_analysis::EmojiEncoding::CustomUrlFallback
    }));
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
});
