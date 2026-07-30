use serde_json::{Value, json};

use super::types::{
    AvatarClass, FirstMessageAssessment, FirstMessageRiskMarker, NewUserAuditAssessment,
    ProfileNameGrammarRelation, SelfReferenceGrammar,
};

pub const REVIEW_RISK_THRESHOLD: i32 = 70;
const FIRST_MESSAGE_SCORE_CAP: i32 = 45;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FirstMessageScoreContext {
    pub template_matches: i32,
    pub spam_similarity: Option<f64>,
    pub feminine_profile_name: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoreComponents {
    pub baseline_score: i32,
    pub baseline_signals: Value,
    pub first_message_score: i32,
    pub first_message_signals: Value,
    pub avatar_score: i32,
    pub avatar_signals: Value,
}

impl ScoreComponents {
    pub fn final_score(&self) -> i32 {
        (self.baseline_score + self.first_message_score + self.avatar_score).clamp(0, 100)
    }

    pub fn final_level(&self) -> &'static str {
        match self.final_score() {
            REVIEW_RISK_THRESHOLD.. => "high",
            40.. => "medium",
            _ => "low",
        }
    }

    pub fn final_signals(&self) -> Value {
        let mut signals = Vec::new();
        for component in [
            &self.baseline_signals,
            &self.first_message_signals,
            &self.avatar_signals,
        ] {
            if let Some(items) = component.as_array() {
                signals.extend(items.iter().cloned());
            }
        }
        Value::Array(signals)
    }
}

pub fn score_assessment(
    baseline_score: i32,
    baseline_signals: Value,
    assessment: &NewUserAuditAssessment,
    first_message_context: FirstMessageScoreContext,
) -> ScoreComponents {
    let (first_message_score, first_message_signals) = assessment
        .first_message_assessment
        .as_ref()
        .map(|assessment| score_first_message(assessment, first_message_context))
        .unwrap_or_else(|| (0, Value::Array(Vec::new())));
    let (avatar_score, avatar_signals) = assessment
        .avatar_observation
        .as_ref()
        .map(score_avatar)
        .unwrap_or_else(|| (0, Value::Array(Vec::new())));

    ScoreComponents {
        baseline_score: baseline_score.clamp(0, 100),
        baseline_signals,
        first_message_score,
        first_message_signals,
        avatar_score,
        avatar_signals,
    }
}

fn score_first_message(
    assessment: &FirstMessageAssessment,
    context: FirstMessageScoreContext,
) -> (i32, Value) {
    let paid_easy_task = has_marker(assessment, FirstMessageRiskMarker::PaidEasyTaskOffer);
    let performative_feminine_persona = context.feminine_profile_name
        && has_marker(
            assessment,
            FirstMessageRiskMarker::PerformativeFemininePersona,
        );
    let llm_score = if paid_easy_task {
        30
    } else if assessment.direct_dm_offer && assessment.offtopic_promo {
        30
    } else if assessment.direct_dm_offer && assessment.template_campaign {
        24
    } else if assessment.template_campaign {
        12
    } else {
        0
    };
    let template_score = i32::from(context.template_matches > 0) * 24;
    let embedding_score = match context.spam_similarity {
        Some(value) if value >= 0.88 => 20,
        Some(value) if value >= 0.78 => 10,
        _ => 0,
    };
    let persona_score = i32::from(performative_feminine_persona) * 12;
    let grammar_conflict = context.feminine_profile_name
        && assessment.self_reference_grammar == SelfReferenceGrammar::Masculine
        && assessment.profile_name_grammar_relation == ProfileNameGrammarRelation::Conflicts;
    let grammar_score = i32::from(grammar_conflict) * 10;
    let score = (llm_score + template_score + embedding_score + persona_score + grammar_score)
        .min(FIRST_MESSAGE_SCORE_CAP);

    let signals = (score > 0).then(|| {
        json!([{
            "class": "first_message_content",
            "label": "first_message_spam_analysis",
            "coefficient": score,
            "warning_strength": if score >= 30 { "strong" } else { "supporting" },
            "assessment": assessment,
            "template_matches": context.template_matches,
            "spam_similarity": context.spam_similarity,
        }])
    });
    (score, signals.unwrap_or_else(|| Value::Array(Vec::new())))
}

fn score_avatar(avatar: &super::types::AvatarObservation) -> (i32, Value) {
    let suggestive_bait = avatar.primary_class == AvatarClass::SuggestiveBait;
    let likely_personal_photo = avatar.primary_class == AvatarClass::OrdinaryPersonal
        && avatar
            .personal_photo_probability
            .is_some_and(|probability| probability >= 0.8);
    let score = i32::from(suggestive_bait) * 8 + i32::from(likely_personal_photo) * 3;
    let signals = (score > 0).then(|| {
        json!([{
            "class": "avatar",
            "label": "unified_avatar_analysis",
            "coefficient": score,
            "warning_strength": "supporting",
            "assessment": avatar,
        }])
    });
    (score, signals.unwrap_or_else(|| Value::Array(Vec::new())))
}

fn has_marker(assessment: &FirstMessageAssessment, marker: FirstMessageRiskMarker) -> bool {
    assessment.risk_markers.contains(&marker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::new_user_audit::types::NewUserAuditAssessment;

    fn assessment(first_message: &str, avatar: &str) -> NewUserAuditAssessment {
        NewUserAuditAssessment::parse(&format!(
            r#"{{
                "avatar_observation": {avatar},
                "first_message_assessment": {first_message},
                "profile_assessment": {{
                    "risk_patterns": ["no_material_risk_pattern"],
                    "evidence": [], "contradictions": ["Нет дополнительных признаков."],
                    "review_priority": "low", "confidence": 0.5, "summary": "Нейтрально."
                }}
            }}"#
        ))
        .unwrap()
    }

    #[test]
    fn first_message_preserves_legacy_cap_and_weights() {
        let assessment = assessment(
            r#"{
                "relation_to_chat":"off_topic", "direct_dm_offer":true,
                "offtopic_promo":true, "template_campaign":true,
                "self_reference_grammar":"none_or_unclear",
                "profile_name_grammar_relation":"not_applicable",
                "risk_markers":["paid_easy_task_offer"], "evidence":[],
                "summary":"Реклама.", "confidence":0.9
            }"#,
            "null",
        );
        let components = score_assessment(
            20,
            json!([]),
            &assessment,
            FirstMessageScoreContext {
                template_matches: 1,
                spam_similarity: Some(0.9),
                feminine_profile_name: false,
            },
        );
        assert_eq!(components.first_message_score, 45);
        assert_eq!(components.final_score(), 65);
    }

    #[test]
    fn avatar_contribution_requires_matching_observation() {
        let assessment = assessment(
            "null",
            r#"{
                "primary_class":"ordinary_personal", "personal_photo_probability":0.8,
                "secondary_classes":[], "face_visibility":"clear", "adult_level":"none",
                "visual_motifs":["портрет"], "description":"Портрет.", "confidence":0.9
            }"#,
        );
        let components = score_assessment(67, json!([]), &assessment, Default::default());
        assert_eq!(components.avatar_score, 3);
        assert_eq!(components.final_score(), REVIEW_RISK_THRESHOLD);
        assert_eq!(components.final_level(), "high");
    }
}
