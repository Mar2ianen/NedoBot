use std::collections::HashMap;

use serde::Deserialize;

/// Линейная TF-IDF модель поверх word 1-2-грам (экспорт `eval/train_nn_spam.py`).
///
/// Токенизация обязана побайтово совпадать со sklearn: lowercase,
/// `(?u)\w+`, ngram 1-2 через пробел, tf — сырой count, idf из экспорта,
/// l2-норма, sigmoid(dot + intercept). Паритет проверяется тестом ниже
/// на реальном сообщении и числе из обучающего скрипта.
#[derive(Debug, Clone)]
pub struct NnSpamModel {
    pub version: String,
    terms: HashMap<String, usize>,
    idf: Vec<f32>,
    coef: Vec<f32>,
    intercept: f32,
}

#[derive(Debug, Deserialize)]
struct NnSpamExport {
    version: String,
    analyzer: String,
    vocab: HashMap<String, f32>,
    coef: Vec<f32>,
    intercept: f32,
}

pub fn load_model(json: &str) -> anyhow::Result<NnSpamModel> {
    let export: NnSpamExport = serde_json::from_str(json)?;
    if export.analyzer != "word_12_lower" {
        anyhow::bail!("unsupported nn spam analyzer {:?}", export.analyzer);
    }
    if export.vocab.len() != export.coef.len() {
        anyhow::bail!(
            "nn spam vocab/coef length mismatch: {} vs {}",
            export.vocab.len(),
            export.coef.len()
        );
    }
    if !export.coef.iter().all(|value| value.is_finite()) || !export.intercept.is_finite() {
        anyhow::bail!("nn spam weights contain non-finite values");
    }
    let mut names: Vec<&String> = export.vocab.keys().collect();
    names.sort();
    // coef порядок соответствует get_feature_names_out (лексикографически),
    // поэтому индекс термина совпадает с позицией в coef.
    let mut terms = HashMap::with_capacity(names.len());
    let mut idf = Vec::with_capacity(names.len());
    for name in names {
        let weight = export.vocab[name];
        if !weight.is_finite() || weight < 0.0 {
            anyhow::bail!("nn spam idf is not a valid weight");
        }
        terms.insert((*name).clone(), idf.len());
        idf.push(weight);
    }
    Ok(NnSpamModel {
        version: export.version,
        terms,
        idf,
        coef: export.coef,
        intercept: export.intercept,
    })
}

fn tokenize(text: &str) -> Vec<String> {
    let words: Vec<String> = text
        .to_lowercase()
        .split(|character: char| !(character.is_alphanumeric() || character == '_'))
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect();
    let mut tokens = Vec::with_capacity(words.len() * 2);
    for word in &words {
        tokens.push(word.clone());
    }
    for pair in words.windows(2) {
        tokens.push(format!("{} {}", pair[0], pair[1]));
    }
    tokens
}

pub fn spam_probability(model: &NnSpamModel, text: &str) -> f64 {
    let mut counts: HashMap<usize, f64> = HashMap::new();
    for token in tokenize(text) {
        if let Some(index) = model.terms.get(&token) {
            *counts.entry(*index).or_default() += 1.0;
        }
    }
    if counts.is_empty() {
        return sigmoid(f64::from(model.intercept));
    }
    let mut norm_sq = 0.0;
    let mut weighted: Vec<(usize, f64)> = Vec::with_capacity(counts.len());
    for (index, count) in counts {
        let value = count * f64::from(model.idf[index]);
        norm_sq += value * value;
        weighted.push((index, value));
    }
    let norm = norm_sq.sqrt();
    let mut dot = f64::from(model.intercept);
    for (index, value) in weighted {
        dot += f64::from(model.coef[index]) * (value / norm);
    }
    sigmoid(dot)
}

fn sigmoid(value: f64) -> f64 {
    1.0 / (1.0 + (-value).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARITY_TEXT: &str = "Недавно ехал за рулем слушал книгу  «Время - деньги». Понравилась тем что она нас учит относиться ко времени и деньгам не как к бесконечному ресурсу, а как к топливу для ракеты: не тратить его на бессмысленную гонку, а мыслить стратегически и фокусироваться на главном. Есть аудио - могу переслать. Пишите в личку, перешлю бесплатно";

    #[test]
    fn tokenization_matches_sklearn_word_12() {
        let tokens = tokenize("Недавно ехал за рулем слушал книгу Время - деньги");
        assert_eq!(
            &tokens[..12],
            &[
                "недавно",
                "ехал",
                "за",
                "рулем",
                "слушал",
                "книгу",
                "время",
                "деньги",
                "недавно ехал",
                "ехал за",
                "за рулем",
                "рулем слушал"
            ]
        );
    }

    #[test]
    fn empty_text_falls_back_to_intercept() {
        let model = tiny_model();
        assert!((spam_probability(&model, "") - sigmoid(0.5)).abs() < 1e-9);
    }

    #[test]
    fn production_model_matches_python_probability() {
        let json = include_str!("../../../models/nn_spam_word12_v1.json");
        let model = load_model(json).expect("production nn model must load");
        let probability = spam_probability(&model, PARITY_TEXT);
        assert!(
            (probability - 0.977_445).abs() < 1e-4,
            "parity drift: {probability}"
        );
    }

    fn tiny_model() -> NnSpamModel {
        // coef идёт в порядке sorted vocab (как get_feature_names_out):
        // ["мир", "привет"] -> [-1.0, 1.0].
        load_model(
            r#"{"version":"test","analyzer":"word_12_lower","vocab":{"привет":1.5,"мир":2.0},"coef":[-1.0,1.0],"intercept":0.5}"#,
        )
        .expect("tiny model must load")
    }

    #[test]
    fn tiny_model_scores_by_hand() {
        // "привет мир": tfidf до нормы [1.5, 2.0], норма 2.5,
        // dot = 1.5/2.5*1.0 + 2.0/2.5*(-1.0) + 0.5 = 0.3
        let model = tiny_model();
        let expected = 1.0 / (1.0 + (-0.3f64).exp());
        assert!((spam_probability(&model, "привет мир") - expected).abs() < 1e-9);
    }

    #[test]
    fn rejects_mismatched_exports() {
        assert!(
            load_model(
                r#"{"version":"x","analyzer":"other","vocab":{},"coef":[],"intercept":0.0}"#
            )
            .is_err()
        );
        assert!(load_model(r#"{"version":"x","analyzer":"word_12_lower","vocab":{"a":1.0},"coef":[],"intercept":0.0}"#).is_err());
    }
}
