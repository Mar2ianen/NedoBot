use std::collections::BTreeSet;

use serde_json::{Value, json};
use sqlx::{PgPool, Row};

use super::types::{
    AvatarClass, EvidenceSource, EvidenceStrength, FirstMessageAssessment, FirstMessageRiskMarker,
    MessageRelation, NewUserAuditAssessment, ProfileNameGrammarRelation, ProfileRiskPattern,
    SelfReferenceGrammar,
};

#[allow(dead_code)]
pub const REVIEW_RISK_THRESHOLD: i32 = 70;
#[allow(dead_code)]
const FIRST_MESSAGE_SCORE_CAP: i32 = 45;
const FIRST_MESSAGE_DECISION_TREE_VERSION: &str = "first-message-tree-v1";

#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FirstMessageScoreContext {
    pub template_matches: i32,
    pub spam_similarity: Option<f64>,
    pub feminine_profile_name: bool,
    pub rkn_vpn_restriction_context: bool,
    pub personal_channel_content: Option<String>,
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
    pub personal_channel_score: i32,
    pub personal_channel_signals: Value,
    pub review_threshold: i32,
}

#[allow(dead_code)]
impl ScoreComponents {
    pub fn final_score(&self) -> i32 {
        self.baseline_score
            .clamp(0, 100)
            .saturating_add(self.first_message_score.clamp(0, 100))
            .saturating_add(self.avatar_score.clamp(0, 100))
            .saturating_add(self.personal_channel_score.clamp(0, 100))
            .clamp(0, 100)
    }

    pub fn final_level(&self) -> &'static str {
        let threshold = self.review_threshold.clamp(0, 100);
        match self.final_score() {
            score if score >= threshold => "high",
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
            &self.personal_channel_signals,
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
    review_threshold: i32,
) -> ScoreComponents {
    let (avatar_score, avatar_signals) = assessment
        .avatar_observation
        .as_ref()
        .map(score_avatar)
        .unwrap_or_else(|| (0, Value::Array(Vec::new())));
    let (personal_channel_score, personal_channel_signals) = score_personal_channel_content(
        &assessment.profile_assessment,
        first_message_context.personal_channel_content.as_deref(),
    );
    let score_before_message = baseline_score.clamp(0, 100).saturating_add(avatar_score);
    let (first_message_score, first_message_signals) = assessment
        .first_message_assessment
        .as_ref()
        .map(|assessment| {
            score_first_message(
                assessment,
                &first_message_context,
                score_before_message,
                review_threshold,
            )
        })
        .unwrap_or_else(|| (0, Value::Array(Vec::new())));

    ScoreComponents {
        baseline_score: baseline_score.clamp(0, 100),
        baseline_signals,
        first_message_score,
        first_message_signals,
        avatar_score,
        avatar_signals,
        personal_channel_score,
        personal_channel_signals,
        review_threshold,
    }
}

fn score_personal_channel_content(
    assessment: &super::types::ProfileAssessment,
    content: Option<&str>,
) -> (i32, Value) {
    let Some(content) = content.filter(|content| !content.trim().is_empty()) else {
        return (0, Value::Array(Vec::new()));
    };
    if assessment.confidence < 0.65
        || !assessment
            .risk_patterns
            .contains(&ProfileRiskPattern::PersonalChannelPromotion)
    {
        return (0, Value::Array(Vec::new()));
    }

    let normalized_content = normalize_channel_evidence(content);
    let grounded_evidence = assessment
        .evidence
        .iter()
        .filter(|evidence| evidence.source == EvidenceSource::PersonalChannel)
        .filter(|evidence| {
            let detail = normalize_channel_evidence(&evidence.detail);
            !detail.is_empty() && normalized_content.contains(&detail)
        })
        .collect::<Vec<_>>();
    let strongest_evidence = grounded_evidence
        .iter()
        .map(|evidence| evidence.strength)
        .max_by_key(|strength| match strength {
            EvidenceStrength::Weak => 0,
            EvidenceStrength::Moderate => 1,
            EvidenceStrength::Strong => 2,
        });
    let score = match (strongest_evidence, assessment.confidence) {
        (Some(EvidenceStrength::Strong), confidence) if confidence >= 0.75 => 22,
        (Some(EvidenceStrength::Strong | EvidenceStrength::Moderate), confidence)
            if confidence >= 0.65 =>
        {
            12
        }
        _ => 0,
    };
    if score == 0 {
        return (0, Value::Array(Vec::new()));
    }

    let signal = json!({
        "class": "llm_profile_bait",
        "label": "llm_personal_channel_content_promotion",
        "coefficient": score,
        "warning_strength": if score >= 20 { "strong" } else { "supporting" },
        "decision": "manual_review",
        "assessment": {
            "confidence": assessment.confidence,
            "risk_pattern": "personal_channel_promotion",
            "evidence": grounded_evidence.iter().map(|evidence| &evidence.detail).collect::<Vec<_>>(),
        },
    });
    (score, json!([signal]))
}

fn normalize_channel_evidence(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character.is_whitespace() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn score_first_message(
    assessment: &FirstMessageAssessment,
    context: &FirstMessageScoreContext,
    score_before_message: i32,
    review_threshold: i32,
) -> (i32, Value) {
    let paid_easy_task = has_marker(assessment, FirstMessageRiskMarker::PaidEasyTaskOffer);
    let rkn_vpn_promotion = context.rkn_vpn_restriction_context
        && assessment.confidence >= 0.85
        && has_marker(assessment, FirstMessageRiskMarker::RknRelatedVpnPromotion)
        && assessment
            .evidence
            .iter()
            .any(|evidence| evidence.marker == FirstMessageRiskMarker::RknRelatedVpnPromotion);
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
    let has_evidence_for = |markers: &[FirstMessageRiskMarker]| {
        assessment
            .risk_markers
            .iter()
            .any(|marker| markers.contains(marker))
            && assessment
                .evidence
                .iter()
                .any(|evidence| markers.contains(&evidence.marker))
    };
    let decisive_direct_dm_funnel = assessment.direct_dm_offer
        && off_topic_promo
        && assessment.confidence >= 0.85
        && has_evidence_for(&[
            FirstMessageRiskMarker::SendOrShareOffer,
            FirstMessageRiskMarker::DirectMessages,
            FirstMessageRiskMarker::SelfHelpOrFinancePromo,
            FirstMessageRiskMarker::ExternalPromoFunnel,
            FirstMessageRiskMarker::PaidEasyTaskOffer,
        ]);
    let decisive_external_promo_funnel = off_topic_promo
        && assessment.confidence >= 0.85
        && has_evidence_for(&[FirstMessageRiskMarker::ExternalPromoFunnel]);
    let decisive_paid_offer = paid_easy_task
        && assessment.confidence >= 0.85
        && has_evidence_for(&[FirstMessageRiskMarker::PaidEasyTaskOffer]);
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
    let rkn_vpn_score = if rkn_vpn_promotion { 35 } else { 0 };
    let known_campaign_match = context.template_matches > 0
        || context
            .spam_similarity
            .is_some_and(|similarity| similarity >= 0.88);
    let supporting_score =
        llm_score + template_score + embedding_score + persona_score + grammar_score;
    let decisive = rkn_vpn_promotion
        || decisive_direct_dm_funnel
        || decisive_external_promo_funnel
        || decisive_paid_offer
        || known_campaign_match;
    let capped_score = (rkn_vpn_score + supporting_score).min(FIRST_MESSAGE_SCORE_CAP);
    let review_floor = review_threshold
        .clamp(0, 100)
        .saturating_sub(score_before_message);
    let available_score = 100_i32.saturating_sub(score_before_message);
    let score = if decisive {
        capped_score.max(review_floor).min(available_score)
    } else {
        capped_score.min(available_score)
    };

    let label = if rkn_vpn_promotion {
        "rkn_vpn_service_promotion"
    } else if decisive_direct_dm_funnel {
        "offtopic_direct_dm_funnel"
    } else if decisive_external_promo_funnel {
        "offtopic_external_promo_funnel"
    } else if decisive_paid_offer {
        "evidence_backed_paid_task_offer"
    } else if known_campaign_match {
        "known_spam_campaign_match"
    } else {
        "unified_first_message_analysis"
    };
    let decision_tree_path = if rkn_vpn_promotion {
        Some(json!([
            "restriction_question_context",
            "vpn_promotion_marker",
            "evidence_quote"
        ]))
    } else if decisive_direct_dm_funnel {
        Some(json!([
            "offtopic_chat_context",
            "direct_dm_offer",
            "evidence_backed_campaign_marker"
        ]))
    } else if decisive_external_promo_funnel {
        Some(json!([
            "offtopic_chat_context",
            "external_promo_funnel_marker",
            "evidence_quote"
        ]))
    } else if decisive_paid_offer {
        Some(json!(["paid_easy_task_offer", "evidence_quote"]))
    } else if known_campaign_match {
        Some(json!(["known_template_or_spam_embedding_match"]))
    } else {
        None
    };
    let mut signals = Vec::new();
    if score > 0 {
        let mut signal = json!({
            "class": "first_message_content",
            "label": label,
            "coefficient": score,
            "warning_strength": if decisive || score >= 30 { "strong" } else { "supporting" },
            "assessment": assessment,
            "template_matches": context.template_matches,
            "spam_similarity": context.spam_similarity,
        });
        if let Some(path) = decision_tree_path {
            signal["decision_tree_version"] = json!(FIRST_MESSAGE_DECISION_TREE_VERSION);
            signal["decision_tree_path"] = path;
        }
        signals.push(signal);
    }
    (score, Value::Array(signals))
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

pub(crate) fn is_rkn_vpn_restriction_context(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "vpn",
        "впн",
        "ркн",
        "роскомнадзор",
        "блокиров",
        "обход огранич",
        "обход блок",
        "не грузит",
        "не загружается",
        "не открывается",
        "недоступ",
    ]
    .iter()
    .any(|marker| text.contains(marker))
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

const SPAM_SIMILARITY_SQL: &str = r#"
    select max(1.0 - (a.first_message_embedding <=> $1::vector))
    from telegram_new_user_profile_audits a
    join telegram_chat_users u
      on u.chat_id = a.chat_id and u.telegram_user_id = a.telegram_user_id
    where u.is_spammer
      and a.first_message_embedding is not null
      and a.telegram_user_id <> $2
    "#;

pub(crate) async fn spam_similarity(
    pool: &PgPool,
    candidate_user_id: i64,
    embedding: &str,
) -> anyhow::Result<Option<f64>> {
    let value = sqlx::query_scalar::<_, Option<f64>>(SPAM_SIMILARITY_SQL)
        .bind(embedding)
        .bind(candidate_user_id)
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
                "self_reference_grammar":"masculine",
                "profile_name_grammar_relation":"conflicts",
                "risk_markers":["paid_easy_task_offer","performative_feminine_persona"], "evidence":[],
                "summary":"Реклама.", "confidence":0.9
            }"#,
            "null",
        );
        let components = score_assessment(
            20,
            json!([]),
            &assessment,
            FirstMessageScoreContext {
                template_matches: 0,
                spam_similarity: None,
                feminine_profile_name: true,
                rkn_vpn_restriction_context: false,
                ..Default::default()
            },
            REVIEW_RISK_THRESHOLD,
        );
        assert_eq!(components.first_message_score, 45);
        assert_eq!(components.final_score(), 65);
    }

    #[test]
    fn final_score_clamps_negative_components_before_adding_them() {
        let components = ScoreComponents {
            baseline_score: 96,
            baseline_signals: json!([]),
            first_message_score: -6,
            first_message_signals: json!([]),
            avatar_score: 0,
            avatar_signals: json!([]),
            personal_channel_score: 0,
            personal_channel_signals: json!([]),
            review_threshold: REVIEW_RISK_THRESHOLD,
        };

        assert_eq!(components.final_score(), 96);
        assert_eq!(components.final_level(), "high");
    }

    #[test]
    fn personal_channel_attachment_without_content_evidence_scores_zero() {
        let assessment = assessment("null", "null");
        let components = score_assessment(
            0,
            json!([]),
            &assessment,
            FirstMessageScoreContext {
                personal_channel_content: Some("Личный дневник о книгах и прогулках".to_string()),
                ..Default::default()
            },
            REVIEW_RISK_THRESHOLD,
        );

        assert_eq!(components.personal_channel_score, 0);
        assert!(
            components
                .personal_channel_signals
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(components.final_score(), 0);
    }

    #[test]
    fn channel_promotion_score_requires_a_grounded_quote_and_promotion_pattern() {
        let mut assessment = assessment("null", "null");
        assessment.profile_assessment.risk_patterns =
            vec![ProfileRiskPattern::PersonalChannelPromotion];
        assessment.profile_assessment.confidence = 0.9;
        assessment.profile_assessment.evidence = vec![super::super::types::AuditEvidence {
            source: EvidenceSource::PersonalChannel,
            detail: "Пишите в личку, отправлю ссылку на VPN".to_string(),
            strength: EvidenceStrength::Strong,
        }];
        let content = "Пишите в личку, отправлю ссылку на VPN";
        let context = FirstMessageScoreContext {
            personal_channel_content: Some(content.to_string()),
            ..Default::default()
        };

        let components = score_assessment(
            0,
            json!([]),
            &assessment,
            context.clone(),
            REVIEW_RISK_THRESHOLD,
        );
        assert_eq!(components.personal_channel_score, 22);
        assert_eq!(components.final_score(), 22);
        assert_eq!(
            components.personal_channel_signals[0]["label"],
            "llm_personal_channel_content_promotion"
        );
        assert_eq!(
            components.personal_channel_signals[0]["decision"],
            "manual_review"
        );

        let ungrounded = score_assessment(
            0,
            json!([]),
            &assessment,
            FirstMessageScoreContext {
                personal_channel_content: Some("Нейтральный пост о книгах".to_string()),
                ..Default::default()
            },
            REVIEW_RISK_THRESHOLD,
        );
        assert_eq!(ungrounded.personal_channel_score, 0);

        assessment.profile_assessment.risk_patterns.clear();
        let no_pattern =
            score_assessment(0, json!([]), &assessment, context, REVIEW_RISK_THRESHOLD);
        assert_eq!(no_pattern.personal_channel_score, 0);
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

        let components = score_assessment(
            0,
            json!([]),
            &assessment,
            Default::default(),
            REVIEW_RISK_THRESHOLD,
        );

        assert_eq!(components.first_message_score, 0);
    }

    #[test]
    fn rkn_vpn_promotion_needs_matching_reply_context_and_confident_evidence() {
        let assessment = assessment(
            r#"{
                "relation_to_chat":"on_topic", "direct_dm_offer":false,
                "offtopic_promo":false, "template_campaign":false,
                "self_reference_grammar":"none_or_unclear",
                "profile_name_grammar_relation":"not_applicable",
                "risk_markers":["rkn_related_vpn_promotion"],
                "evidence":[{"marker":"rkn_related_vpn_promotion","quote":"Попробуйте мой VPN"}],
                "summary":"Продвигает VPN-сервис.", "confidence":0.91
            }"#,
            "null",
        );
        let context = FirstMessageScoreContext {
            rkn_vpn_restriction_context: true,
            ..Default::default()
        };
        let components = score_assessment(
            0,
            json!([]),
            &assessment,
            context.clone(),
            REVIEW_RISK_THRESHOLD,
        );
        assert_eq!(components.first_message_score, REVIEW_RISK_THRESHOLD);
        assert_eq!(components.final_score(), REVIEW_RISK_THRESHOLD);
        assert_eq!(
            components.first_message_signals[0]["label"],
            "rkn_vpn_service_promotion"
        );

        let unrelated = score_assessment(
            0,
            json!([]),
            &assessment,
            FirstMessageScoreContext::default(),
            REVIEW_RISK_THRESHOLD,
        );
        assert_eq!(unrelated.first_message_score, 0);

        let mut low_confidence = assessment.clone();
        low_confidence
            .first_message_assessment
            .as_mut()
            .unwrap()
            .confidence = 0.84;
        let weak = score_assessment(
            0,
            json!([]),
            &low_confidence,
            context,
            REVIEW_RISK_THRESHOLD,
        );
        assert_eq!(weak.first_message_score, 0);
    }

    #[test]
    fn evidence_backed_offtopic_external_promo_reaches_review_threshold_without_dm_offer() {
        let assessment = assessment(
            r#"{
                "relation_to_chat":"off_topic", "direct_dm_offer":false,
                "offtopic_promo":true, "template_campaign":false,
                "self_reference_grammar":"none_or_unclear",
                "profile_name_grammar_relation":"not_applicable",
                "risk_markers":["external_promo_funnel"],
                "evidence":[{"marker":"external_promo_funnel","quote":"Bellmaster @xjoso2kzizbot bestarve 👍"}],
                "summary":"Вне-тематическая реклама стороннего бота.",
                "confidence":0.91
            }"#,
            "null",
        );

        let components = score_assessment(
            45,
            json!([]),
            &assessment,
            FirstMessageScoreContext::default(),
            REVIEW_RISK_THRESHOLD,
        );

        assert_eq!(components.first_message_score, 25);
        assert_eq!(components.final_score(), REVIEW_RISK_THRESHOLD);
        assert_eq!(
            components.first_message_signals[0]["label"],
            "offtopic_external_promo_funnel"
        );
        assert_eq!(
            components.first_message_signals[0]["decision_tree_path"],
            json!([
                "offtopic_chat_context",
                "external_promo_funnel_marker",
                "evidence_quote"
            ])
        );

        let mut on_topic = assessment.clone();
        let on_topic_message = on_topic.first_message_assessment.as_mut().unwrap();
        on_topic_message.relation_to_chat = MessageRelation::OnTopic;
        on_topic_message.offtopic_promo = false;
        let topical = score_assessment(
            45,
            json!([]),
            &on_topic,
            FirstMessageScoreContext::default(),
            REVIEW_RISK_THRESHOLD,
        );
        assert_eq!(topical.first_message_score, 0);
        assert_eq!(topical.final_score(), 45);

        let mut low_confidence = assessment;
        low_confidence
            .first_message_assessment
            .as_mut()
            .unwrap()
            .confidence = 0.84;
        let weak = score_assessment(
            45,
            json!([]),
            &low_confidence,
            FirstMessageScoreContext::default(),
            REVIEW_RISK_THRESHOLD,
        );
        assert_eq!(weak.first_message_score, 0);
        assert_eq!(weak.final_score(), 45);
    }

    #[test]
    fn evidence_backed_offtopic_dm_funnel_reaches_review_threshold() {
        let assessment = assessment(
            r#"{
                "relation_to_chat":"off_topic", "direct_dm_offer":true,
                "offtopic_promo":true, "template_campaign":false,
                "self_reference_grammar":"none_or_unclear",
                "profile_name_grammar_relation":"not_applicable",
                "risk_markers":["send_or_share_offer","direct_messages"],
                "evidence":[{"marker":"send_or_share_offer","quote":"Есть аудиоверсия, пишите в личку, отправлю."}],
                "summary":"Вне-тематическое предложение прислать материал в личку.",
                "confidence":0.91
            }"#,
            "null",
        );

        let components = score_assessment(
            29,
            json!([]),
            &assessment,
            FirstMessageScoreContext::default(),
            REVIEW_RISK_THRESHOLD,
        );

        assert_eq!(components.first_message_score, 41);
        assert_eq!(components.final_score(), REVIEW_RISK_THRESHOLD);
        assert_eq!(
            components.first_message_signals[0]["label"],
            "offtopic_direct_dm_funnel"
        );
        assert_eq!(
            components.first_message_signals[0]["warning_strength"],
            "strong"
        );
    }

    #[test]
    fn known_campaign_tree_match_reaches_review_threshold() {
        let assessment = assessment(
            r#"{
                "relation_to_chat":"on_topic", "direct_dm_offer":false,
                "offtopic_promo":false, "template_campaign":false,
                "self_reference_grammar":"none_or_unclear",
                "profile_name_grammar_relation":"not_applicable",
                "risk_markers":[], "evidence":[],
                "summary":"Обычное сообщение.", "confidence":0.5
            }"#,
            "null",
        );
        let components = score_assessment(
            0,
            json!([]),
            &assessment,
            FirstMessageScoreContext {
                template_matches: 1,
                ..Default::default()
            },
            REVIEW_RISK_THRESHOLD,
        );

        assert_eq!(components.final_score(), REVIEW_RISK_THRESHOLD);
        assert_eq!(
            components.first_message_signals[0]["label"],
            "known_spam_campaign_match"
        );
        assert_eq!(
            components.first_message_signals[0]["decision_tree_version"],
            FIRST_MESSAGE_DECISION_TREE_VERSION
        );
    }

    #[test]
    fn spam_similarity_query_excludes_the_candidate_users_own_messages() {
        assert!(SPAM_SIMILARITY_SQL.contains("a.telegram_user_id <> $2"));
    }

    #[test]
    fn identifies_rkn_and_vpn_discussion_context_without_generic_failure_phrases() {
        assert!(is_rkn_vpn_restriction_context(
            "Какой ВПН работает после блокировки?"
        ));
        assert!(is_rkn_vpn_restriction_context(
            "После действий Роскомнадзора сайт не грузит"
        ));
        assert!(!is_rkn_vpn_restriction_context(
            "Приложение у меня сегодня не работает"
        ));
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
        let components = score_assessment(
            67,
            json!([]),
            &assessment,
            Default::default(),
            REVIEW_RISK_THRESHOLD,
        );
        assert_eq!(components.avatar_score, 3);
        assert_eq!(components.final_score(), REVIEW_RISK_THRESHOLD);
        assert_eq!(components.final_level(), "high");
    }

    #[test]
    fn final_level_uses_the_job_review_threshold() {
        let assessment = assessment("null", "null");
        let components = score_assessment(65, json!([]), &assessment, Default::default(), 60);

        assert_eq!(components.final_score(), 65);
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
