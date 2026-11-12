//! Ограниченные структурные наблюдения, отдельно от версии TF-IDF model_text.
use serde::{Deserialize, Serialize};
use teloxide_antispam::preprocess::prepare_text;

pub const VERSION: &str = "spam-structure-v1";
pub const MAX_INPUT_CHARS: usize = 4_096;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StructuralFeatures {
    pub version: String,
    pub truncated: bool,
    pub space_letter_ratio: f64,
    pub single_letter_token_ratio: f64,
    pub max_single_letter_run: usize,
    pub compact_changed_token_count: usize,
    pub mixed_script_token_count: usize,
    pub invisible_between_letters: usize,
    pub obfuscation_score: u32,
}

pub fn extract(text: &str) -> StructuralFeatures {
    let bounded: String = text.chars().take(MAX_INPUT_CHARS).collect();
    let prepared = prepare_text(&bounded);
    let mut tokens = 0;
    let mut single = 0;
    let mut run = 0;
    let mut max_run = 0;
    for token in prepared.canonical.split(|c: char| !c.is_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        tokens += 1;
        if token.chars().count() == 1 && token.chars().all(char::is_alphabetic) {
            single += 1;
            run += 1;
            max_run = max_run.max(run);
        } else {
            run = 0;
        }
    }
    let mut letters = 0;
    let mut spaces = 0;
    let mut invisibles = 0;
    let mut previous_letter = false;
    let mut pending_invisible = 0;
    for c in bounded.chars() {
        letters += usize::from(c.is_alphabetic());
        spaces += usize::from(c.is_whitespace());
        if matches!(
            c,
            '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}' | '\u{034f}' | '\u{00ad}'
        ) {
            if previous_letter {
                pending_invisible += 1;
            }
        } else {
            if c.is_alphabetic() {
                invisibles += pending_invisible;
            }
            previous_letter = c.is_alphabetic();
            pending_invisible = 0;
        }
    }
    let compact_changed = prepared.compact_variant.as_ref().map_or(0, |c| {
        prepared
            .canonical
            .split_whitespace()
            .count()
            .saturating_sub(c.split_whitespace().count())
    });
    // Наблюдение об обфускации, а не вероятность спама и не основание санкций.
    let score = (u32::from(max_run >= 5) * 35
        + u32::from(invisibles > 0) * 25
        + u32::from(prepared.flags.mixed_script_words > 0) * 20
        + u32::from(compact_changed > 0) * 20)
        .min(100);
    StructuralFeatures {
        version: VERSION.into(),
        truncated: text.chars().nth(MAX_INPUT_CHARS).is_some(),
        space_letter_ratio: spaces as f64 / letters.max(1) as f64,
        single_letter_token_ratio: single as f64 / tokens.max(1) as f64,
        max_single_letter_run: max_run,
        compact_changed_token_count: compact_changed,
        mixed_script_token_count: prepared.flags.mixed_script_words,
        invisible_between_letters: invisibles,
        obfuscation_score: score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observes_structure_without_rewriting_model_input() {
        let f = extract("Есть П О Д Р А Б О Т К А, ПИШИ МНЕ!");
        assert_eq!(f.max_single_letter_run, 10);
        assert!(f.compact_changed_token_count >= 9);
        assert!(f.single_letter_token_ratio > 0.5);
        assert_eq!(extract("работа 👩‍💻").invisible_between_letters, 0);
        assert_eq!(
            extract("ра\u{200b}б\u{200d}ота").invisible_between_letters,
            2
        );
        assert!(extract(&"а".repeat(MAX_INPUT_CHARS + 1)).truncated);
    }
}
