use std::collections::BTreeSet;

use serde_json::{Value, json};
use sqlx::{PgPool, Row};

use super::types::{
    AvatarClass, FirstMessageAssessment, FirstMessageRiskMarker, MessageRelation,
    NewUserAuditAssessment, ProfileNameGrammarRelation, SelfReferenceGrammar,
};

#[allow(dead_code)]
pub const REVIEW_RISK_THRESHOLD: i32 = 70;
#[allow(dead_code)]
const FIRST_MESSAGE_SCORE_CAP: i32 = 45;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FirstMessageScoreContext {
    pub template_matches: i32,
    pub spam_similarity: Option<f64>,
    pub feminine_profile_name: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreComponents {
    pub baseline_score: i32,
    pub baseline_signals: Value,
    pub first_message_score: i32,
    pub first_message_signals: Value,
    pub avatar_score: i32,
    pub avatar_signals: Value,
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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
    let off_topic_promo = assessment.offtopic_promo
        && matches!(
            assessment.relation_to_chat,
            MessageRelation::LooselyRelated | MessageRelation::OffTopic
        );
    let llm_score = if paid_easy_task || (assessment.direct_dm_offer && off_topic_promo) {
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
            "label": "unified_first_message_analysis",
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

pub(crate) async fn template_match_count(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
    text: &str,
) -> anyhow::Result<i32> {
    let rows = sqlx::query(
        r#"
        select distinct m.text
        from telegram_messages m
        where m.chat_id = $1
          and m.spam_marked_at is not null
          and m.user_id <> $2
          and m.text is not null
        "#,
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    let current = token_set(text);
    Ok(rows
        .into_iter()
        .filter_map(|row| row.get::<Option<String>, _>("text"))
        .filter(|candidate| jaccard(&current, &token_set(candidate)) >= 0.5)
        .count()
        .min(10) as i32)
}

pub(crate) async fn spam_similarity(pool: &PgPool, embedding: &str) -> anyhow::Result<Option<f64>> {
    let value = sqlx::query_scalar::<_, Option<f64>>(
        r#"
        select max(1.0 - (a.first_message_embedding <=> $1::vector))
        from telegram_new_user_profile_audits a
        join telegram_chat_users u
          on u.chat_id = a.chat_id and u.telegram_user_id = a.telegram_user_id
        where u.is_spammer and a.first_message_embedding is not null
        "#,
    )
    .bind(embedding)
    .fetch_one(pool)
    .await?;
    Ok(value)
}

fn token_set(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.chars().count() >= 4)
        .map(campaign_token)
        .collect()
}

fn campaign_token(word: &str) -> String {
    match word {
        "отправить"
        | "отправлю"
        | "переслать"
        | "перешлю"
        | "скинуть"
        | "скину"
        | "поделиться"
        | "поделюсь"
        | "закинуть"
        | "закину" => "send_offer".to_string(),
        "личку" | "личные" | "сообщения" | "стучитесь" => {
            "direct_messages".to_string()
        }
        "аудиокнигу" | "аудиокнига" | "аудиоверсия" | "текстовая" => {
            "promoted_material".to_string()
        }
        _ => word.to_owned(),
    }
}

fn jaccard(left: &BTreeSet<String>, right: &BTreeSet<String>) -> f64 {
    let union = left.union(right).count();
    if union == 0 {
        0.0
    } else {
        left.intersection(right).count() as f64 / union as f64
    }
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
    fn first_message_preserves_unified_cap_and_weights() {
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
    fn on_topic_offtopic_promo_does_not_add_direct_message_score() {
        let mut assessment = assessment(
            r#"{
                "relation_to_chat":"on_topic", "direct_dm_offer":true,
                "offtopic_promo":false, "template_campaign":false,
                "self_reference_grammar":"none_or_unclear",
                "profile_name_grammar_relation":"not_applicable",
                "risk_markers":[], "evidence":[],
                "summary":"Тематическое сообщение.", "confidence":0.9
            }"#,
            "null",
        );
        assessment
            .first_message_assessment
            .as_mut()
            .expect("test assessment must contain first message")
            .offtopic_promo = true;

        let components = score_assessment(0, json!([]), &assessment, Default::default());

        assert_eq!(components.first_message_score, 0);
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

    #[test]
    fn template_similarity_catches_campaign_variants() {
        assert!(
            jaccard(
                &token_set("могу переслать аудиокнигу пишите в личку"),
                &token_set("есть аудиоверсия могу отправить пишите в личные сообщения")
            ) >= 0.4
        );
    }
}
