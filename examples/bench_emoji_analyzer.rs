use std::hint::black_box;
use std::time::Instant;

use tg_ai_bot_teloxide::features::emoji_analysis::{MAX_TEXT_BYTES, analyze};

fn measure(label: &str, text: &str, iterations: usize) {
    let mut occurrences = 0usize;
    let mut samples = Vec::with_capacity(7);
    for _ in 0..7 {
        let started = Instant::now();
        for _ in 0..iterations {
            let analysis = analyze(black_box(text), &[]).expect("bounded benchmark text");
            occurrences = occurrences.saturating_add(analysis.occurrences.len());
            black_box(analysis.metrics);
        }
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let elapsed = samples[3];
    let bytes = text.len().saturating_mul(iterations);
    println!(
        "{label}: {} bytes, median of 7 x {iterations} iterations: {:?}, {:.1} MiB/s, {occurrences} occurrences across all samples",
        text.len(),
        elapsed,
        bytes as f64 / elapsed.as_secs_f64() / (1024.0 * 1024.0),
    );
}

fn main() {
    let seeds = [
        "ASCII words and punctuation: one, two, three. ",
        "Привет, мир! Unicode текст с кириллицей и латиницей. ",
        "👩‍💻 ❤️ 🏳️‍🌈 👨‍👩‍👧‍👦 👍🏽 😂 ",
        "क्‍षि हिन्दी grapheme conjuncts; e\u{301} decomposed mark. ",
    ];
    let mut text = String::new();
    while text.len() + seeds[0].len() <= 32 * 1024 {
        text.push_str(seeds[text.len() % seeds.len()]);
    }
    assert!(text.len() <= MAX_TEXT_BYTES);
    measure("mixed", &text, 1_000);

    let emoji_heavy = "👩‍💻❤️🏳️‍🌈👨‍👩‍👧‍👦👍🏽😂".repeat(2_000);
    measure("emoji-heavy", &emoji_heavy, 1_000);

    let long_ascii = "ordinary text without emoji ".repeat(9_362);
    measure("long-ascii", &long_ascii, 100);
}
