//! Воспроизводимая текстовая диагностика без изменения policy или вызова провайдера.
use super::{deterministic, structure};
use serde::Serialize;
use serde_json::{Value, json};
use teloxide_antispam::preprocess::{PREPROCESSING_VERSION, prepare_text};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissedStage {
    NotIngested,
    NotEligible,
    Normalization,
    Features,
    Classification,
    Policy,
    Delivery,
    Failed,
    NoConfirmedMiss,
}

pub fn newcomer_limits() -> (i64, i64) {
    deterministic::text_stream_limits()
}

pub fn analyze_text(text: &str, count: i64, age_seconds: Option<i64>) -> Value {
    let bounded: String = text.chars().take(structure::MAX_INPUT_CHARS).collect();
    let prepared = prepare_text(&bounded);
    let campaign = deterministic::classify(Some(&bounded), count, age_seconds, false);
    json!({"eligible":age_seconds.is_some_and(|age|deterministic::text_stream_eligible(count,age)),"normalization_version":PREPROCESSING_VERSION,"canonical":prepared.canonical,
        "compact_variant":prepared.compact_variant,"normalization_flags":prepared.flags,
        "structure":structure::extract(&bounded),"rule_version":deterministic::RULE_VERSION,
        "lexical_baseline_candidate":campaign.as_ref().is_some_and(|c|c.can_short_circuit_audit()),
        "local_plus_candidate":campaign.is_some(),"campaign":campaign.as_ref().map(|c|c.score_signal()),
        "new_auto_ban_authority":false})
}

#[cfg(test)]
mod tests {
    use super::*;
    const CORPUS: &str = include_str!("../../../tests/fixtures/antispam_v2.json");
    #[test]
    fn mandatory_regressions_and_related_unicode_variants() {
        let corpus: Value = serde_json::from_str(CORPUS).unwrap();
        for case in corpus["cases"].as_array().unwrap() {
            let text = case["text"].as_str().unwrap();
            // Связанные трансформации, не независимые статистические наблюдения.
            let mut variants = vec![text.to_owned(), text.to_uppercase(), text.to_lowercase()];
            for whitespace in [
                "\u{a0}", "\u{202f}", "\t", "\n", "\r\n", "\u{2009}", "\u{3000}",
            ] {
                variants.push(text.replace(' ', whitespace));
                variants.push(text.to_uppercase().replace(' ', whitespace));
            }
            for invisible in ['\u{200b}', '\u{2060}', '\u{feff}', '\u{00ad}'] {
                variants.push(
                    text.chars()
                        .flat_map(|c| {
                            if c.is_alphabetic() {
                                vec![c, invisible]
                            } else {
                                vec![c]
                            }
                        })
                        .collect(),
                );
            }
            variants.push(text.replace('о', "o").replace('а', "a").replace('е', "e"));
            variants.push(text.replace(' ', "\u{a0}").replace('о', "o\u{200b}"));
            variants.push(text.replace('!', "!!!").replace(',', ",,,"));
            assert!(variants.len() >= 24);
            for variant in variants {
                let trace = analyze_text(&variant, 1, Some(60));
                assert_eq!(
                    trace["local_plus_candidate"], case["review"],
                    "{}: {variant}",
                    case["id"]
                );
                assert_eq!(trace["new_auto_ban_authority"], false);
            }
        }
    }
    #[test]
    fn eligibility_and_truncation_remain_bounded() {
        for (count, age) in [(11, Some(0)), (1, None), (1, Some(259_201)), (1, Some(-1))] {
            assert_eq!(
                analyze_text("Есть подработка, пиши мне", count, age)["local_plus_candidate"],
                false
            );
        }
        assert_eq!(
            analyze_text("Привет", 1, Some(0))["local_plus_candidate"],
            false
        );
    }
}
