//! Вероятности Decision преобразуются в content-вклад, не в прямой ban verdict.
use super::{
    client::{Answer, DecisionAssessment, ShadowVerdict},
    contacts::ContactFeatures,
};
use crate::config_file::DecisionConfig;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub const VERSION: &str = "decision-score-v1";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contribution {
    pub version: String,
    pub decision_score: i32,
    pub contact_score: i32,
    pub review_candidate: bool,
    pub semantic_probability: f64,
}
pub fn contribution(
    a: &DecisionAssessment,
    contacts: &ContactFeatures,
    cfg: &DecisionConfig,
) -> Contribution {
    let probability = |key: &str| match a.answers.get(key) {
        Some(Answer::Noul { noul, .. }) => *noul,
        _ => 0.0,
    };
    let candidate = a.captcha_verdict() == ShadowVerdict::ReviewCandidate;
    // Conservative rubric composition: marginal probabilities не перемножаем.
    let semantic = probability("unsolicited_promotion")
        .max(probability("work_solicitation").min(probability("private_contact_funnel")));
    Contribution {
        version: VERSION.into(),
        decision_score: if candidate {
            (semantic * f64::from(cfg.score_weight)).round() as i32
        } else {
            0
        },
        contact_score: if contacts.external_candidate() {
            cfg.external_contact_weight
        } else {
            0
        },
        review_candidate: candidate,
        semantic_probability: semantic,
    }
}
impl Contribution {
    /// Только безопасные explainability-метки: raw answers/контакты остаются private.
    pub fn decision_signals(&self) -> Value {
        if self.decision_score == 0 {
            return json!([]);
        }
        json!([{"class":"decision_content","label":"decision_promotion","score":self.decision_score,
            "coefficient":self.decision_score,"version":VERSION,"warning_strength":"strong","auto_ban_eligible":false}])
    }
    pub fn contact_signals(&self) -> Value {
        if self.contact_score == 0 {
            return json!([]);
        }
        json!([{"class":"contact_context","label":"outside_observed_chat_contact","coefficient":self.contact_score,
            "warning_strength":"supporting","membership_claim":"not_known_to_be_current_member","auto_ban_eligible":false}])
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contact_alone_and_quotes_do_not_authorize_captcha() {
        let mut a: DecisionAssessment = serde_json::from_str(include_str!(
            "../../../tests/fixtures/decision_response.json"
        ))
        .unwrap();
        let contacts = ContactFeatures {
            unseen_usernames: 1,
            ..Default::default()
        };
        let c = contribution(&a, &contacts, &DecisionConfig::default());
        assert!(!c.review_candidate);
        assert_eq!(c.decision_score, 0);
        assert_eq!(c.contact_score, 8);
        a.answers.insert(
            "unsolicited_promotion".into(),
            Answer::Noul {
                noul: 0.99,
                confidence: 0.99,
            },
        );
        a.answers.insert(
            "quoted_or_discussed_spam".into(),
            Answer::Noul {
                noul: 0.9,
                confidence: 0.9,
            },
        );
        let c = contribution(&a, &contacts, &DecisionConfig::default());
        assert!(!c.review_candidate);
        assert_eq!(c.decision_score, 0);
    }
}
