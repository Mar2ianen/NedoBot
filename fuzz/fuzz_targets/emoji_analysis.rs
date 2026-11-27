#![no_main]

use libfuzzer_sys::fuzz_target;
use tg_ai_bot_teloxide::features::emoji_analysis::{analyze, MAX_TEXT_BYTES};

fuzz_target!(|data: &[u8]| {
    let bounded = &data[..data.len().min(MAX_TEXT_BYTES + 1)];
    let text = String::from_utf8_lossy(bounded);
    if let Ok(analysis) = analyze(&text, &[]) {
        for occurrence in analysis.occurrences {
            assert!(text.is_char_boundary(occurrence.utf8_range.start));
            assert!(text.is_char_boundary(occurrence.utf8_range.end));
            assert_eq!(
                &text[occurrence.utf8_range.clone()],
                occurrence.fallback
            );
            assert_eq!(
                text[..occurrence.utf8_range.start].encode_utf16().count(),
                occurrence.utf16_range.start
            );
            assert_eq!(
                text[..occurrence.utf8_range.end].encode_utf16().count(),
                occurrence.utf16_range.end
            );
        }
    }
});
