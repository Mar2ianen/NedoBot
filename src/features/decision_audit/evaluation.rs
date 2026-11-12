//! Замороженное A/B/C/D сравнение. Без обучения, сети, записей или sanctions.
use super::client::{Answer, DecisionAssessment, POLICY_VERSION, QUESTIONS, ShadowVerdict};
use crate::features::new_user_audit::{diagnostics::analyze_text, local_cascade::hash_bytes};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenExample {
    pub instance_id: String,
    /// Общий frozen context (policy/avatar/доступные данные), не только текст.
    pub context_hash: String,
    pub author_group: String,
    pub campaign_group: String,
    pub partition: String,
    pub message_at: DateTime<Utc>,
    pub label_at: DateTime<Utc>,
    pub text: String,
    /// Только отдельно проверенная message label; author stamps оставляют None.
    pub message_label: Option<bool>,
    pub message_count: i64,
    pub chat_age_seconds: Option<i64>,
    pub manual_not_spam: bool,
    /// Реальное замороженное решение baseline, не новый пересчёт score.
    pub baseline_candidate: bool,
    pub decision: Option<FrozenDecision>,
    #[serde(default)]
    pub question_labels: BTreeMap<String, bool>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenDecision {
    pub context_hash: String,
    pub source_hash: String,
    pub question_version: String,
    pub normalization_version: String,
    pub policy_version: String,
    pub model: String,
    pub assessment: Option<DecisionAssessment>,
    pub error_kind: Option<String>,
    pub api_latency_ms: Option<u64>,
    pub queue_latency_ms: Option<u64>,
    pub action_latency_ms: Option<u64>,
}
#[derive(Default, Serialize)]
struct Metrics {
    samples: usize,
    positives: usize,
    negatives: usize,
    tp: usize,
    fp: usize,
    candidates: usize,
    fn_by_category: BTreeMap<String, usize>,
    normal_authors: BTreeSet<String>,
    normal_campaigns: BTreeSet<String>,
    budget_tp: BTreeMap<usize, usize>,
}
impl Metrics {
    fn add(&mut self, r: &FrozenExample, candidate: bool) {
        self.candidates += usize::from(candidate);
        if candidate && r.message_label == Some(true) {
            for budget in [5, 10, 25, 50] {
                if self.candidates <= budget {
                    *self.budget_tp.entry(budget).or_default() += 1;
                }
            }
        }
        if let Some(label) = r.message_label {
            self.samples += 1;
            self.positives += usize::from(label);
            self.negatives += usize::from(!label);
            if !label {
                self.normal_authors.insert(r.author_group.clone());
                self.normal_campaigns.insert(r.campaign_group.clone());
            }
            self.tp += usize::from(label && candidate);
            self.fp += usize::from(!label && candidate);
            if label && !candidate {
                *self
                    .fn_by_category
                    .entry(r.campaign_group.clone())
                    .or_default() += 1;
            }
        }
    }
    fn report(&self) -> Value {
        let ratio = |n: usize, d: usize| {
            if d > 0 {
                Some(n as f64 / d as f64)
            } else {
                None
            }
        };
        let budgets: BTreeMap<String, Option<f64>> = [5, 10, 25, 50]
            .into_iter()
            .map(|b| {
                (
                    b.to_string(),
                    ratio(self.budget_tp.get(&b).copied().unwrap_or(0), self.positives),
                )
            })
            .collect();
        json!({"recall_at_review_budget_in_arrival_order":budgets,"counts":self,"recall":ratio(self.tp,self.positives),"fpr":ratio(self.fp,self.negatives),
            "precision":ratio(self.tp,self.tp+self.fp),"zero_fp_upper_95_approx":if self.fp==0&&self.negatives>0 {Some((3.0/self.normal_authors.len().min(self.normal_campaigns.len()).max(1) as f64).min(1.0))}else{None}})
    }
}
fn quantiles(values: &mut [u64]) -> Value {
    values.sort_unstable();
    if values.is_empty() {
        return Value::Null;
    }
    let at = |p: f64| {
        values[((values.len() as f64 * p).ceil() as usize)
            .saturating_sub(1)
            .min(values.len() - 1)]
    };
    json!({"n":values.len(),"p50_ms":at(0.5),"p95_ms":at(0.95),"p99_ms":at(0.99)})
}

pub fn compare(rows: &[FrozenExample], instance: &str, model: &str) -> anyhow::Result<Value> {
    let mut authors = BTreeMap::<&str, &str>::new();
    let mut campaigns = BTreeMap::<&str, &str>::new();
    let mut counts: [Metrics; 4] = std::array::from_fn(|_| Metrics::default());
    let mut api = Vec::new();
    let mut queue = Vec::new();
    let mut action = Vec::new();
    let mut calls = 0;
    let mut provider_errors = 0;
    let mut abstentions = 0;
    let mut tokens = 0u64;
    let mut calibration = BTreeMap::<String, Vec<(f64, bool)>>::new();
    let mut unique_sources = BTreeSet::new();
    let mut ordered: Vec<_> = rows.iter().collect();
    ordered.sort_by_key(|r| (r.message_at, hash_bytes(r.text.as_bytes())));
    for row in ordered {
        anyhow::ensure!(row.instance_id == instance, "mixed instance dataset");
        anyhow::ensure!(
            row.context_hash.len() == 64 && row.context_hash.chars().all(|c| c.is_ascii_hexdigit()),
            "invalid frozen context hash"
        );
        anyhow::ensure!(row.message_at <= row.label_at, "label precedes message");
        anyhow::ensure!(
            !row.author_group.is_empty() && !row.campaign_group.is_empty(),
            "missing grouping"
        );
        for (groups, key) in [
            (&mut authors, row.author_group.as_str()),
            (&mut campaigns, row.campaign_group.as_str()),
        ] {
            let previous = groups.insert(key, &row.partition);
            anyhow::ensure!(
                previous.is_none_or(|p| p == row.partition),
                "author/campaign partition leakage"
            );
        }
        let hash = hash_bytes(row.text.as_bytes());
        anyhow::ensure!(
            unique_sources.insert((row.author_group.as_str(), hash.clone())),
            "duplicate/related source must not inflate independent samples"
        );
        let mut decision_candidate = false;
        if let Some(d) = &row.decision {
            anyhow::ensure!(
                d.context_hash == row.context_hash
                    && d.source_hash == hash
                    && d.question_version == hash_bytes(QUESTIONS.as_bytes())
                    && d.policy_version == POLICY_VERSION
                    && d.normalization_version
                        == teloxide_antispam::preprocess::PREPROCESSING_VERSION
                    && d.model == model,
                "decision snapshot/version drift"
            );
            calls += 1;
            if let Some(a) = &d.assessment {
                anyhow::ensure!(
                    d.error_kind.is_none() && a.validate().is_ok(),
                    "invalid frozen assessment"
                );
                decision_candidate = a.verdict() == ShadowVerdict::ReviewCandidate;
                abstentions += usize::from(a.verdict() == ShadowVerdict::Abstain);
                tokens = tokens.saturating_add(a.usage.total_input_tokens);
                for (question, label) in &row.question_labels {
                    let Some(Answer::Noul { noul, .. }) = a.answers.get(question) else {
                        anyhow::bail!("question label must name a noul target");
                    };
                    calibration
                        .entry(question.clone())
                        .or_default()
                        .push((*noul, *label));
                }
            } else {
                provider_errors += 1;
                abstentions += 1;
            }
            if let Some(v) = d.api_latency_ms {
                api.push(v);
            }
            if let Some(v) = d.queue_latency_ms {
                queue.push(v);
            }
            if let Some(v) = d.action_latency_ms {
                action.push(v);
            }
        } else {
            abstentions += 1;
        }
        let local = analyze_text(&row.text, row.message_count, row.chat_age_seconds)["local_plus_candidate"]
            == true;
        let candidates = [
            row.baseline_candidate,
            row.baseline_candidate || local,
            row.baseline_candidate || decision_candidate,
            row.baseline_candidate || local || decision_candidate,
        ];
        for (metric, candidate) in counts.iter_mut().zip(candidates) {
            metric.add(row, candidate && !row.manual_not_spam);
        }
    }
    let calibration: Value = calibration
        .into_iter()
        .map(|(q, values)| {
            let n = values.len() as f64;
            let brier = values
                .iter()
                .map(|(p, y)| (p - f64::from(*y)).powi(2))
                .sum::<f64>()
                / n;
            let mut ece = 0.0;
            for bin in 0..10 {
                let points: Vec<_> = values
                    .iter()
                    .filter(|(p, _)| ((*p * 10.0) as usize).min(9) == bin)
                    .collect();
                if !points.is_empty() {
                    let mean = points.iter().map(|(p, _)| p).sum::<f64>() / points.len() as f64;
                    let target = points.iter().map(|(_, y)| f64::from(*y)).sum::<f64>()
                        / points.len() as f64;
                    ece += points.len() as f64 / n * (mean - target).abs();
                }
            }
            (q, json!({"n":values.len(),"brier":brier,"ece_10_bins":ece}))
        })
        .collect();
    Ok(
        json!({"instance":instance,"snapshot_hash":hash_bytes(&serde_json::to_vec(rows)?),"rows":rows.len(),
        "variants":{"A_baseline":counts[0].report(),"B_local_plus":counts[1].report(),"C_decision_shadow":counts[2].report(),"D_combined":counts[3].report()},
        "decision_calls":calls,"calls_per_1000_evaluated_messages":if rows.is_empty(){None}else{Some(calls as f64/rows.len() as f64*1000.0)},
        "provider_error_rate":if calls>0{Some(provider_errors as f64/calls as f64)}else{None},"abstentions_or_unavailable":abstentions,
        "decision_coverage_complete":calls==rows.len(),"input_tokens":tokens,"cost_usd":null,
        "api_latency":quantiles(&mut api),"queue_latency":quantiles(&mut queue),"action_latency":quantiles(&mut action),"question_calibration":calibration,
        "note":"Candidate precision/recall, not ban accuracy. API errors abstain and retain baseline; missing message labels excluded. No probability training or threshold promotion. Related Unicode variants must remain a separate robustness suite."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(text: &str, author: &str, label: Option<bool>) -> FrozenExample {
        FrozenExample {
            instance_id: "fixture".into(),
            context_hash: hash_bytes(text.as_bytes()),
            author_group: author.into(),
            campaign_group: author.into(),
            partition: "holdout".into(),
            message_at: DateTime::from_timestamp(1, 0).unwrap(),
            label_at: DateTime::from_timestamp(2, 0).unwrap(),
            text: text.into(),
            message_label: label,
            message_count: 1,
            chat_age_seconds: Some(0),
            manual_not_spam: false,
            baseline_candidate: false,
            decision: None,
            question_labels: BTreeMap::new(),
        }
    }
    #[test]
    fn same_snapshots_unknown_labels_and_unavailable_decisions_do_not_fabricate_accuracy() {
        let rows = vec![
            row("Есть подработка, пиши мне", "a", Some(true)),
            row("GPU RTX 5090", "b", Some(false)),
            row("Неразмеченный текст", "c", None),
        ];
        let report = compare(&rows, "fixture", "model").unwrap();
        assert_eq!(report["variants"]["B_local_plus"]["counts"]["samples"], 2);
        assert_eq!(report["variants"]["B_local_plus"]["counts"]["tp"], 1);
        assert_eq!(report["variants"]["B_local_plus"]["counts"]["fp"], 0);
        assert_eq!(report["decision_coverage_complete"], false);
        assert_eq!(report["abstentions_or_unavailable"], 3);
        assert_eq!(report["api_latency"], Value::Null);
        assert_eq!(report["variants"]["C_decision_shadow"]["recall"], 0.0);
    }
    #[test]
    fn rejects_cross_instance_partition_and_version_leakage() {
        let mut a = row("text", "a", None);
        a.instance_id = "foreign".into();
        assert!(compare(&[a], "fixture", "model").is_err());
        let a = row("text", "a", None);
        let mut b = row("other", "a", None);
        b.partition = "train".into();
        assert!(compare(&[a, b], "fixture", "model").is_err());
        let mut a = row("text", "a", Some(false));
        a.decision = Some(FrozenDecision {
            context_hash: a.context_hash.clone(),
            source_hash: "wrong".into(),
            question_version: hash_bytes(QUESTIONS.as_bytes()),
            normalization_version: teloxide_antispam::preprocess::PREPROCESSING_VERSION.into(),
            policy_version: POLICY_VERSION.into(),
            model: "model".into(),
            assessment: None,
            error_kind: Some("timeout".into()),
            api_latency_ms: None,
            queue_latency_ms: None,
            action_latency_ms: None,
        });
        assert!(compare(&[a], "fixture", "model").is_err());
    }
    #[test]
    fn identical_text_does_not_allow_reusing_a_different_context_assessment() {
        let mut a = row("text", "a", Some(false));
        a.decision = Some(FrozenDecision {
            context_hash: hash_bytes(b"other-policy-or-avatar"),
            source_hash: hash_bytes(a.text.as_bytes()),
            question_version: hash_bytes(QUESTIONS.as_bytes()),
            normalization_version: teloxide_antispam::preprocess::PREPROCESSING_VERSION.into(),
            policy_version: POLICY_VERSION.into(),
            model: "model".into(),
            assessment: None,
            error_kind: Some("timeout".into()),
            api_latency_ms: None,
            queue_latency_ms: None,
            action_latency_ms: None,
        });
        assert!(compare(&[a], "fixture", "model").is_err());
    }
}
