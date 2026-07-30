use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Лимиты применит worker после подключения в следующем slice.
#[allow(dead_code)]
const MAX_LIST_ITEMS: usize = 8;
#[allow(dead_code)]
const MAX_TEXT_LENGTH: usize = 600;
#[allow(dead_code)]
const MAX_EVIDENCE_ITEMS: usize = 10;

/// Строгий результат единой LLM-оценки нового участника.
///
/// Поля с наблюдениями за аватаром и первым сообщением всегда присутствуют в
/// JSON, но равны `null`, если соответствующих входных данных не было.
#[allow(dead_code)] // Typed-контракт ожидает будущий worker аудита.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NewUserAuditAssessment {
    pub avatar_observation: Option<AvatarObservation>,
    pub first_message_assessment: Option<FirstMessageAssessment>,
    pub profile_assessment: ProfileAssessment,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AvatarObservation {
    pub primary_class: AvatarClass,
    pub personal_photo_probability: Option<f64>,
    pub secondary_classes: Vec<AvatarClass>,
    pub face_visibility: FaceVisibility,
    pub adult_level: AdultLevel,
    pub visual_motifs: Vec<String>,
    pub description: String,
    pub confidence: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AvatarClass {
    OrdinaryPersonal,
    MotivationalStock,
    PolishedPersona,
    SuggestiveBait,
    ExplicitAdult,
    CommercialOrScam,
    IllustrationOrCharacter,
    Unclear,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FaceVisibility {
    Clear,
    Partial,
    None,
    Unclear,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AdultLevel {
    None,
    Suggestive,
    Explicit,
    Unclear,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FirstMessageAssessment {
    pub relation_to_chat: MessageRelation,
    pub direct_dm_offer: bool,
    pub offtopic_promo: bool,
    pub template_campaign: bool,
    pub self_reference_grammar: SelfReferenceGrammar,
    pub profile_name_grammar_relation: ProfileNameGrammarRelation,
    pub risk_markers: Vec<FirstMessageRiskMarker>,
    pub evidence: Vec<FirstMessageEvidence>,
    pub summary: String,
    pub confidence: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRelation {
    OnTopic,
    LooselyRelated,
    OffTopic,
    NoMessageContext,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SelfReferenceGrammar {
    Masculine,
    Feminine,
    NoneOrUnclear,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileNameGrammarRelation {
    Consistent,
    Conflicts,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FirstMessageRiskMarker {
    SendOrShareOffer,
    DirectMessages,
    SelfHelpOrFinancePromo,
    TemplateEfficiencyNarrative,
    MaskedCallToAction,
    PaidEasyTaskOffer,
    ExternalPromoFunnel,
    GenericCampaignReaction,
    PerformativeFemininePersona,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FirstMessageEvidence {
    pub marker: FirstMessageRiskMarker,
    pub quote: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProfileAssessment {
    pub risk_patterns: Vec<ProfileRiskPattern>,
    pub evidence: Vec<AuditEvidence>,
    pub contradictions: Vec<String>,
    pub review_priority: ReviewPriority,
    pub confidence: f64,
    pub summary: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileRiskPattern {
    BioOrUsernamePromotion,
    PersonalChannelPromotion,
    CommercialOrScamPresentation,
    CrossSourceInconsistency,
    RepeatedSpamPattern,
    NoMaterialRiskPattern,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AuditEvidence {
    pub source: EvidenceSource,
    pub detail: String,
    pub strength: EvidenceStrength,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    Avatar,
    Profile,
    FirstMessage,
    PersonalChannel,
    ChatHistory,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStrength {
    Weak,
    Moderate,
    Strong,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPriority {
    Low,
    Medium,
    High,
}

#[allow(dead_code)] // Вызов parse/validate появится вместе с worker-ом аудита.
impl NewUserAuditAssessment {
    /// Десериализует ответ модели и отвергает формально допустимые, но опасные
    /// для хранения или модерации значения.
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        Self::parse_for_input(value, true)
    }

    /// Проверяет ответ с учётом того, был ли в снимке доступный для анализа аватар.
    pub fn parse_for_input(value: &str, has_avatar_input: bool) -> anyhow::Result<Self> {
        Self::parse_for_modalities(value, has_avatar_input, false)
    }

    pub fn parse_for_modalities(
        value: &str,
        has_avatar_input: bool,
        has_first_message_input: bool,
    ) -> anyhow::Result<Self> {
        let value: Value =
            serde_json::from_str(value).context("LLM audit output is not valid JSON")?;
        let object = value
            .as_object()
            .context("LLM audit output must be a JSON object")?;
        for field in [
            "avatar_observation",
            "first_message_assessment",
            "profile_assessment",
        ] {
            if !object.contains_key(field) {
                bail!("LLM audit output is missing required field {field}");
            }
        }

        let assessment: Self = serde_json::from_value(value)
            .context("LLM audit output does not match the assessment contract")?;
        assessment.validate_for_modalities(has_avatar_input, has_first_message_input)?;
        Ok(assessment)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_for_input(true)
    }

    pub fn validate_for_input(&self, has_avatar_input: bool) -> anyhow::Result<()> {
        self.validate_for_modalities(has_avatar_input, false)
    }

    pub fn validate_for_modalities(
        &self,
        has_avatar_input: bool,
        has_first_message_input: bool,
    ) -> anyhow::Result<()> {
        if !has_avatar_input && self.avatar_observation.is_some() {
            bail!("avatar_observation must be null when the audit input has no avatar");
        }
        if has_first_message_input && self.first_message_assessment.is_none() {
            bail!(
                "first_message_assessment must be present when the audit input has a first message"
            );
        }
        validate_avatar(self.avatar_observation.as_ref())?;
        validate_first_message(self.first_message_assessment.as_ref())?;
        validate_profile(&self.profile_assessment)
    }
}

fn validate_avatar(avatar: Option<&AvatarObservation>) -> anyhow::Result<()> {
    let Some(avatar) = avatar else { return Ok(()) };
    validate_list(
        "avatar_observation.secondary_classes",
        &avatar.secondary_classes,
    )?;
    validate_text_list("avatar_observation.visual_motifs", &avatar.visual_motifs)?;
    validate_text("avatar_observation.description", &avatar.description)?;
    if let Some(probability) = avatar.personal_photo_probability {
        validate_probability("avatar_observation.personal_photo_probability", probability)?;
    }
    validate_probability("avatar_observation.confidence", avatar.confidence)
}

fn validate_first_message(assessment: Option<&FirstMessageAssessment>) -> anyhow::Result<()> {
    let Some(assessment) = assessment else {
        return Ok(());
    };
    validate_list(
        "first_message_assessment.risk_markers",
        &assessment.risk_markers,
    )?;
    if assessment.evidence.len() > MAX_EVIDENCE_ITEMS {
        bail!("first_message_assessment.evidence exceeds {MAX_EVIDENCE_ITEMS} items");
    }
    for evidence in &assessment.evidence {
        validate_text("first_message_assessment.evidence.quote", &evidence.quote)?;
    }
    validate_text("first_message_assessment.summary", &assessment.summary)?;
    validate_probability("first_message_assessment.confidence", assessment.confidence)
}

fn validate_profile(assessment: &ProfileAssessment) -> anyhow::Result<()> {
    validate_list(
        "profile_assessment.risk_patterns",
        &assessment.risk_patterns,
    )?;
    if assessment.evidence.len() > MAX_EVIDENCE_ITEMS {
        bail!("profile_assessment.evidence exceeds {MAX_EVIDENCE_ITEMS} items");
    }
    for evidence in &assessment.evidence {
        validate_text("profile_assessment.evidence.detail", &evidence.detail)?;
    }
    validate_text_list(
        "profile_assessment.contradictions",
        &assessment.contradictions,
    )?;
    validate_text("profile_assessment.summary", &assessment.summary)?;
    validate_probability("profile_assessment.confidence", assessment.confidence)
}

fn validate_list<T>(field: &str, values: &[T]) -> anyhow::Result<()> {
    if values.len() > MAX_LIST_ITEMS {
        bail!("{field} exceeds {MAX_LIST_ITEMS} items");
    }
    Ok(())
}

fn validate_text_list(field: &str, values: &[String]) -> anyhow::Result<()> {
    validate_list(field, values)?;
    for value in values {
        validate_text(field, value)?;
    }
    Ok(())
}

fn validate_text(field: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty() {
        bail!("{field} must not be empty");
    }
    if value.chars().count() > MAX_TEXT_LENGTH {
        bail!("{field} exceeds {MAX_TEXT_LENGTH} characters");
    }
    if value.contains('<') || value.contains('>') {
        bail!("{field} must not contain raw HTML");
    }
    Ok(())
}

fn validate_probability(field: &str, value: f64) -> anyhow::Result<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        bail!("{field} must be a finite number between 0 and 1");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_ASSESSMENT: &str = r#"{
        "avatar_observation": null,
        "first_message_assessment": null,
        "profile_assessment": {
            "risk_patterns": ["no_material_risk_pattern"],
            "evidence": [{"source": "profile", "detail": "Биография не содержит рекламы.", "strength": "weak"}],
            "contradictions": ["Нет независимых признаков спама."],
            "review_priority": "low",
            "confidence": 0.75,
            "summary": "Оснований для приоритетной проверки нет."
        }
    }"#;

    #[test]
    fn parse_requires_nullable_sections_and_profile_assessment() {
        let assessment = NewUserAuditAssessment::parse(VALID_ASSESSMENT).unwrap();
        assert_eq!(assessment.avatar_observation, None);
        assert_eq!(assessment.first_message_assessment, None);

        let missing = VALID_ASSESSMENT.replace("\"avatar_observation\": null,", "");
        assert!(NewUserAuditAssessment::parse(&missing).is_err());
    }

    #[test]
    fn parse_rejects_unknown_fields_and_unknown_enum_values() {
        let unknown_field = VALID_ASSESSMENT.replace(
            "\"confidence\": 0.75,",
            "\"confidence\": 0.75, \"unexpected\": true,",
        );
        assert!(NewUserAuditAssessment::parse(&unknown_field).is_err());

        let unknown_enum = VALID_ASSESSMENT.replace("\"low\"", "\"urgent\"");
        assert!(NewUserAuditAssessment::parse(&unknown_enum).is_err());
    }

    #[test]
    fn parse_rejects_html_and_out_of_range_confidence() {
        let html = VALID_ASSESSMENT.replace(
            "Оснований для приоритетной проверки нет.",
            "<b>Небезопасный вывод</b>",
        );
        assert!(NewUserAuditAssessment::parse(&html).is_err());

        let invalid_confidence = VALID_ASSESSMENT.replace("0.75", "1.1");
        assert!(NewUserAuditAssessment::parse(&invalid_confidence).is_err());
    }
}
