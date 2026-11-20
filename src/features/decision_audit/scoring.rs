//! Вероятности Decision преобразуются в content-вклад, не в прямой ban verdict.
use super::{
    client::{Answer, DecisionAssessment, ShadowVerdict},
    contacts::ContactFeatures,
};
use crate::config_file::DecisionConfig;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub const VERSION: &str = "decision-score-v2";
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewBasis {
    #[default]
    None,
    Promotion,
    OfftopicWithContext,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contribution {
    pub version: String,
    pub decision_score: i32,
    pub contact_score: i32,
    pub review_candidate: bool,
    pub semantic_probability: f64,
    #[serde(default)]
    pub review_basis: ReviewBasis,
}
pub fn contribution(
    a: &DecisionAssessment,
    contacts: &ContactFeatures,
    cfg: &DecisionConfig,
) -> Contribution {
    contribution_with_context(a, contacts, cfg, &Value::Null)
}

pub fn contribution_with_context(
    a: &DecisionAssessment,
    contacts: &ContactFeatures,
    cfg: &DecisionConfig,
    input: &Value,
) -> Contribution {
    let probability = |key: &str| match a.answers.get(key) {
        Some(Answer::Noul { noul, .. }) => *noul,
        _ => 0.0,
    };
    let promotion = a.captcha_verdict() == ShadowVerdict::ReviewCandidate;
    let off_topic = a.off_topic_context_candidate(input);
    let candidate = promotion || off_topic;
    // Conservative rubric composition: marginal probabilities не перемножаем.
    let semantic = if off_topic {
        probability("offtopic_message")
    } else {
        probability("unsolicited_promotion")
            .max(probability("work_solicitation").min(probability("private_contact_funnel")))
    };
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
        review_basis: if promotion {
            ReviewBasis::Promotion
        } else if off_topic {
            ReviewBasis::OfftopicWithContext
        } else {
            ReviewBasis::None
        },
    }
}
impl Contribution {
    /// Только безопасные explainability-метки: raw answers/контакты остаются private.
    pub fn decision_signals(&self) -> Value {
        if self.decision_score == 0 {
            return json!([]);
        }
        let label = match self.review_basis {
            ReviewBasis::Promotion => "decision_promotion",
            ReviewBasis::OfftopicWithContext => "decision_offtopic_with_context",
            ReviewBasis::None => "decision_review",
        };
        json!([{"class":"decision_content","label":label,"score":self.decision_score,
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
    fn off_topic_alone_has_no_spam_or_captcha_authority() {
        let mut body: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/decision_response.json"
        ))
        .unwrap();
        body["answers"]["offtopic_message"]["noul"] = json!(0.99);
        let assessment: DecisionAssessment = serde_json::from_value(body).unwrap();
        assert!(assessment.validate().is_ok());
        assert_eq!(assessment.captcha_verdict(), ShadowVerdict::NoSignal);
        let c = contribution(
            &assessment,
            &ContactFeatures::default(),
            &DecisionConfig::default(),
        );
        assert!(!c.review_candidate);
        assert_eq!(c.decision_score, 0);
    }
    #[test]
    fn implicit_adult_promotion_can_qualify_for_captcha_review() {
        let mut body: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/decision_response.json"
        ))
        .unwrap();
        body["answers"]["unsolicited_promotion"]["noul"] = json!(0.96);
        body["answers"]["contextual_relevance"]["score"] = json!(0.0);
        body["answers"]["contextual_relevance"]["probabilities"] =
            json!({"0": 1.0, "1": 0.0, "2": 0.0});
        body["answers"]["spam_campaign_type"]["choice"] = json!("adult_promotion");
        body["answers"]["spam_campaign_type"]["probabilities"] = json!({
            "adult_promotion": 0.97, "none": 0.03, "work_dm_bait": 0.0,
            "financial_promotion": 0.0, "other_promotion": 0.0
        });
        let decision: DecisionAssessment = serde_json::from_value(body.clone()).unwrap();
        assert!(decision.validate().is_ok());
        assert_eq!(decision.captcha_verdict(), ShadowVerdict::ReviewCandidate);
        let c = contribution(
            &decision,
            &ContactFeatures::default(),
            &DecisionConfig::default(),
        );
        assert!(c.review_candidate);
        assert_eq!(c.decision_score, 86);

        // Синтетический ответ с низкой вероятностью рекламы: это тест scoring,
        // не проверка распознавания текста/изображения внешней моделью.
        body["answers"]["unsolicited_promotion"]["noul"] = json!(0.1);
        let normal: DecisionAssessment = serde_json::from_value(body).unwrap();
        assert!(normal.validate().is_ok());
        assert_eq!(normal.captcha_verdict(), ShadowVerdict::NoSignal);
    }

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
