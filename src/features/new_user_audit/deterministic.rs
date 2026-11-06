use std::{collections::HashSet, sync::OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use teloxide_antispam::{preprocess::prepare_text, text::normalize_cyrillic_homoglyphs};

pub(crate) const RULE_VERSION: &str = "new-user-features-v2";
const FEATURE_CONFIG: &str = include_str!("spam_features.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CampaignKind {
    MoneyDmFunnel,
    PaidShortTaskOffer,
    PaidWorkOpportunity,
    FinancialOffer,
    LimitedProductTrial,
    ContentShareFunnel,
    AdultPersonalChannel,
}

#[derive(Debug, Clone, Deserialize)]
struct FeatureDefinition {
    name: String,
    weight: i32,
    markers: Vec<String>,
    #[serde(default)]
    matcher: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CampaignDefinition {
    rule: String,
    kind: CampaignKind,
    #[serde(default)]
    auto_ban_eligible: bool,
    subtype: String,
    reason: String,
    priority: i32,
    minimum_score: i32,
    require_all: Vec<String>,
    require_any: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct FeatureConfig {
    version: String,
    max_messages: i64,
    max_chat_age_seconds: i64,
    features: Vec<FeatureDefinition>,
    campaigns: Vec<CampaignDefinition>,
}

static CONFIG: OnceLock<FeatureConfig> = OnceLock::new();

fn config() -> &'static FeatureConfig {
    CONFIG.get_or_init(|| {
        let mut config: FeatureConfig = toml::from_str(FEATURE_CONFIG)
            .expect("embedded new-user spam feature configuration must be valid");
        validate_config(&config)
            .expect("embedded new-user spam feature configuration must be coherent");
        config
            .campaigns
            .sort_by_key(|campaign| std::cmp::Reverse(campaign.priority));
        config
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CampaignMatch {
    pub kind: CampaignKind,
    pub rule: String,
    pub rule_version: String,
    pub feature_config_version: String,
    pub auto_ban_eligible: bool,
    pub subtype: String,
    pub reason: String,
    pub feature_score: i32,
    pub matched_features: Vec<String>,
}

impl CampaignMatch {
    pub fn from_rule(rule: &str) -> Option<Self> {
        if rule == "fresh_adult_personal_channel" {
            return Some(Self {
                kind: CampaignKind::AdultPersonalChannel,
                rule: rule.to_owned(),
                rule_version: RULE_VERSION.to_owned(),
                feature_config_version: config().version.clone(),
                auto_ban_eligible: false,
                subtype: "adult_personal_channel_promo".to_owned(),
                reason: "У свежего участника личный канал содержит ссылки на adult-промо."
                    .to_owned(),
                feature_score: 100,
                matched_features: vec!["personal_channel_adult_links".to_owned()],
            });
        }
        config()
            .campaigns
            .iter()
            .find(|campaign| campaign.rule == rule)
            .map(|campaign| Self::from_definition(campaign, 0, Vec::new()))
    }

    pub fn from_stored_evidence(rule: &str, evidence: Option<&Value>) -> Option<Self> {
        let mut campaign = evidence
            .and_then(|evidence| {
                Some(Self {
                    kind: serde_json::from_value(evidence.get("kind")?.clone()).ok()?,
                    rule: evidence
                        .get("rule")
                        .and_then(Value::as_str)
                        .unwrap_or(rule)
                        .to_owned(),
                    rule_version: evidence
                        .get("rule_version")
                        .and_then(Value::as_str)
                        .unwrap_or(RULE_VERSION)
                        .to_owned(),
                    feature_config_version: evidence
                        .get("feature_config_version")
                        .and_then(Value::as_str)
                        .unwrap_or(&config().version)
                        .to_owned(),
                    auto_ban_eligible: evidence
                        .get("auto_ban_eligible")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    subtype: evidence.get("subtype")?.as_str()?.to_owned(),
                    reason: evidence.get("reason")?.as_str()?.to_owned(),
                    feature_score: evidence
                        .get("feature_score")
                        .and_then(Value::as_i64)
                        .unwrap_or_default() as i32,
                    matched_features: evidence
                        .get("matched_features")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                })
            })
            .or_else(|| Self::from_rule(rule))?;
        if let Some(evidence) = evidence {
            campaign.feature_score = evidence
                .get("feature_score")
                .and_then(Value::as_i64)
                .unwrap_or_default() as i32;
            campaign.matched_features = evidence
                .get("matched_features")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            // Signals from older builds did not contain a confidence tier;
            // fail closed for auto-ban while retaining the persisted wording.
            campaign.auto_ban_eligible = evidence
                .get("auto_ban_eligible")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        }
        Some(campaign)
    }

    fn from_definition(
        campaign: &CampaignDefinition,
        feature_score: i32,
        matched_features: Vec<String>,
    ) -> Self {
        Self {
            kind: campaign.kind,
            rule: campaign.rule.clone(),
            rule_version: RULE_VERSION.to_owned(),
            feature_config_version: config().version.clone(),
            auto_ban_eligible: campaign.auto_ban_eligible,
            subtype: campaign.subtype.clone(),
            reason: campaign.reason.clone(),
            feature_score,
            matched_features,
        }
    }

    pub fn assessment(&self, has_first_message: bool) -> Value {
        if !self.auto_ban_eligible {
            return self.review_only_assessment(has_first_message);
        }
        let first_message_assessment = if !has_first_message {
            Value::Null
        } else if self.kind == CampaignKind::MoneyDmFunnel {
            json!({
                "relation_to_chat": "off_topic",
                "direct_dm_offer": true,
                "offtopic_promo": true,
                "template_campaign": true,
                "self_reference_grammar": "none_or_unclear",
                "profile_name_grammar_relation": "not_applicable",
                "risk_markers": ["direct_messages", "self_help_or_finance_promo"],
                "evidence": [
                    {"marker": "direct_messages", "quote": "Комбинация денежного или рабочего предложения с призывом продолжить контакт отдельно от чата."},
                    {"marker": "self_help_or_finance_promo", "quote": self.reason}
                ],
                "summary": self.reason,
                "confidence": 1.0
            })
        } else if matches!(
            self.kind,
            CampaignKind::PaidShortTaskOffer | CampaignKind::PaidWorkOpportunity
        ) {
            json!({
                "relation_to_chat": "off_topic",
                "direct_dm_offer": false,
                "offtopic_promo": true,
                "template_campaign": true,
                "self_reference_grammar": "none_or_unclear",
                "profile_name_grammar_relation": "not_applicable",
                "risk_markers": ["paid_easy_task_offer"],
                "evidence": [{"marker": "paid_easy_task_offer", "quote": self.reason}],
                "summary": self.reason,
                "confidence": 1.0
            })
        } else if self.kind == CampaignKind::FinancialOffer {
            json!({
                "relation_to_chat": "off_topic",
                "direct_dm_offer": false,
                "offtopic_promo": true,
                "template_campaign": true,
                "self_reference_grammar": "none_or_unclear",
                "profile_name_grammar_relation": "not_applicable",
                "risk_markers": ["self_help_or_finance_promo"],
                "evidence": [{"marker": "self_help_or_finance_promo", "quote": self.reason}],
                "summary": self.reason,
                "confidence": 1.0
            })
        } else if self.kind == CampaignKind::ContentShareFunnel {
            json!({
                "relation_to_chat": "off_topic",
                "direct_dm_offer": true,
                "offtopic_promo": true,
                "template_campaign": true,
                "self_reference_grammar": "none_or_unclear",
                "profile_name_grammar_relation": "not_applicable",
                "risk_markers": ["direct_messages", "send_or_share_offer"],
                "evidence": [{"marker": "send_or_share_offer", "quote": self.reason}],
                "summary": self.reason,
                "confidence": 1.0
            })
        } else if self.kind == CampaignKind::LimitedProductTrial {
            json!({
                "relation_to_chat": "off_topic",
                "direct_dm_offer": false,
                "offtopic_promo": true,
                "template_campaign": true,
                "self_reference_grammar": "none_or_unclear",
                "profile_name_grammar_relation": "not_applicable",
                "risk_markers": ["send_or_share_offer"],
                "evidence": [{"marker": "send_or_share_offer", "quote": self.reason}],
                "summary": self.reason,
                "confidence": 1.0
            })
        } else {
            json!({
                "relation_to_chat": "no_message_context",
                "direct_dm_offer": false,
                "offtopic_promo": false,
                "template_campaign": false,
                "self_reference_grammar": "none_or_unclear",
                "profile_name_grammar_relation": "not_applicable",
                "risk_markers": [],
                "evidence": [],
                "summary": self.reason,
                "confidence": 1.0
            })
        };

        let evidence_source = match self.kind {
            CampaignKind::MoneyDmFunnel
            | CampaignKind::PaidShortTaskOffer
            | CampaignKind::LimitedProductTrial => "first_message",
            CampaignKind::PaidWorkOpportunity
            | CampaignKind::FinancialOffer
            | CampaignKind::ContentShareFunnel => "first_message",
            CampaignKind::AdultPersonalChannel => "personal_channel",
        };
        json!({
            "avatar_observation": null,
            "first_message_assessment": first_message_assessment,
            "profile_assessment": {
                "risk_patterns": ["commercial_or_scam_presentation"],
                "evidence": [{
                    "source": evidence_source,
                    "detail": self.reason,
                    "strength": "strong"
                }],
                "contradictions": [],
                "review_priority": "high",
                "confidence": 1.0,
                "summary": self.reason
            }
        })
    }

    fn review_only_assessment(&self, has_first_message: bool) -> Value {
        let first_message_assessment = has_first_message.then(|| {
            json!({
                "relation_to_chat": "loosely_related",
                "direct_dm_offer": false,
                "offtopic_promo": false,
                "template_campaign": false,
                "self_reference_grammar": "none_or_unclear",
                "profile_name_grammar_relation": "not_applicable",
                "risk_markers": [],
                "evidence": [],
                "summary": self.reason,
                "confidence": 0.4
            })
        });
        json!({
            "avatar_observation": null,
            "first_message_assessment": first_message_assessment,
            "profile_assessment": {
                "risk_patterns": [],
                "evidence": [],
                "contradictions": [],
                "review_priority": "medium",
                "confidence": 0.4,
                "summary": self.reason
            }
        })
    }

    pub fn score_signal(&self) -> Value {
        let label = if self.auto_ban_eligible {
            "deterministic_campaign_fast_path"
        } else {
            "deterministic_campaign_review_candidate"
        };
        json!({
            "class": "deterministic_campaign",
            "label": label,
            "auto_ban_eligible": self.auto_ban_eligible,
            "kind": self.kind,
            "subtype": self.subtype,
            "reason": self.reason,
            "rule": self.rule,
            "rule_version": self.rule_version,
            "feature_config_version": self.feature_config_version,
            "feature_score": self.feature_score,
            "matched_features": self.matched_features,
            "decision": if self.auto_ban_eligible { "auto_spam" } else { "manual_review" },
            "warning_strength": if self.auto_ban_eligible { "strong" } else { "supporting" }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExtractedFeatures {
    matched: Vec<String>,
    score: i32,
}

impl ExtractedFeatures {
    fn contains(&self, feature: &str) -> bool {
        self.matched.iter().any(|matched| matched == feature)
    }

    fn satisfies(&self, campaign: &CampaignDefinition) -> bool {
        campaign
            .require_all
            .iter()
            .all(|feature| self.contains(feature))
            && campaign
                .require_any
                .iter()
                .all(|group| group.iter().any(|feature| self.contains(feature)))
            && self.score >= campaign.minimum_score
    }
}

pub(crate) fn classify(
    first_message: Option<&str>,
    message_count: i64,
    chat_age_seconds: Option<i64>,
    personal_channel_has_adult_links: bool,
) -> Option<CampaignMatch> {
    let config = config();
    let chat_age_seconds = chat_age_seconds?;
    if message_count > config.max_messages
        || !(0..=config.max_chat_age_seconds).contains(&chat_age_seconds)
    {
        return None;
    }

    if personal_channel_has_adult_links {
        return CampaignMatch::from_rule("fresh_adult_personal_channel");
    }

    let normalized = first_message?;
    let normalized = normalize_mixed_script_homoglyphs(normalized);
    let normalized = prepare_text(&normalized).model_text();
    let features = extract_features(&normalized, config);
    config
        .campaigns
        .iter()
        .find(|campaign| features.satisfies(campaign))
        .map(|campaign| CampaignMatch::from_definition(campaign, features.score, features.matched))
}

fn extract_features(text: &str, config: &FeatureConfig) -> ExtractedFeatures {
    let matching_features: Vec<_> = config
        .features
        .iter()
        .filter(|feature| {
            feature
                .markers
                .iter()
                .any(|marker| contains_feature_marker(text, marker))
                || feature
                    .matcher
                    .as_deref()
                    .is_some_and(|matcher| matches_feature_matcher(text, matcher))
        })
        .collect();
    ExtractedFeatures {
        matched: matching_features
            .iter()
            .map(|feature| feature.name.clone())
            .collect(),
        score: matching_features.iter().map(|feature| feature.weight).sum(),
    }
}

fn contains_feature_marker(text: &str, marker: &str) -> bool {
    text.match_indices(marker).any(|(index, _)| {
        let prefix = &text[..index];
        let starts_at_word = index == 0
            || prefix
                .chars()
                .next_back()
                .is_none_or(|previous| !previous.is_alphanumeric());
        let immediately_negated = prefix.split_whitespace().next_back() == Some("не");
        starts_at_word && !immediately_negated
    })
}

fn matches_feature_matcher(text: &str, matcher: &str) -> bool {
    match matcher {
        "concrete_ruble_amount" => has_concrete_ruble_amount(text),
        "large_number" => numeric_amounts(text)
            .into_iter()
            .any(|(amount, _)| amount >= 100),
        _ => false,
    }
}

fn normalize_mixed_script_homoglyphs(text: &str) -> String {
    let visually_mapped = normalize_cyrillic_homoglyphs(text);
    visually_mapped
        .split_inclusive(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(normalize_mixed_token)
        .collect()
}

fn normalize_mixed_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    let has_cyrillic = chars.iter().any(|ch| is_cyrillic(*ch));
    let latin_count = chars.iter().filter(|ch| ch.is_ascii_alphabetic()).count();
    let cyrillic_count = chars.iter().filter(|ch| is_cyrillic(**ch)).count();
    let translate_mixed_word = has_cyrillic && latin_count <= cyrillic_count + 3;
    chars
        .iter()
        .enumerate()
        .map(|(index, ch)| {
            if matches!(ch, 'b' | 'B')
                && index > 0
                && index + 1 < chars.len()
                && is_cyrillic(chars[index - 1])
                && is_cyrillic(chars[index + 1])
            {
                return if *ch == 'B' { 'Ь' } else { 'ь' };
            }
            if !translate_mixed_word || !ch.is_ascii_alphabetic() {
                return *ch;
            }
            match ch.to_ascii_lowercase() {
                'a' => 'а',
                'b' => 'б',
                'c' => 'с',
                'd' => 'д',
                'e' => 'е',
                'f' => 'ф',
                'g' => 'г',
                'h' => 'х',
                'i' => 'и',
                'j' => 'й',
                'k' => 'к',
                'l' => 'л',
                'm' => 'м',
                'n' => 'н',
                'o' => 'о',
                'p' => 'п',
                'q' => 'к',
                'r' => 'р',
                's' => 'с',
                't' => 'т',
                'u' => 'у',
                'v' => 'в',
                'w' => 'в',
                'x' => 'х',
                'y' => 'ы',
                'z' => 'з',
                _ => *ch,
            }
        })
        .collect()
}

fn is_cyrillic(ch: char) -> bool {
    matches!(ch, '\u{0400}'..='\u{04ff}')
}

fn has_concrete_ruble_amount(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    numeric_amounts(text)
        .into_iter()
        .any(|(amount, end)| amount >= 100 && is_ruble_suffix(&chars, end))
}

fn numeric_amounts(text: &str) -> Vec<(u64, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut amounts = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if !chars[index].is_ascii_digit() {
            index += 1;
            continue;
        }

        let mut end = index;
        let mut digits = String::new();
        while end < chars.len() {
            let ch = chars[end];
            if ch.is_ascii_digit() {
                digits.push(ch);
                end += 1;
                continue;
            }
            if is_ocr_zero(ch) {
                digits.push('0');
                end += 1;
                continue;
            }
            if matches!(ch, '.' | ',' | ' ')
                && chars
                    .get(end + 1)
                    .is_some_and(|next| next.is_ascii_digit() || is_ocr_zero(*next))
            {
                end += 1;
                continue;
            }
            break;
        }
        if let Ok(amount) = digits.parse::<u64>() {
            amounts.push((amount, end));
        }
        index = end.max(index + 1);
    }
    amounts
}

fn is_ocr_zero(ch: char) -> bool {
    matches!(ch, 'o' | 'O' | 'о' | 'О')
}

fn is_ruble_suffix(chars: &[char], mut index: usize) -> bool {
    while chars.get(index).is_some_and(|ch| ch.is_whitespace()) {
        index += 1;
    }
    if chars.get(index) == Some(&'₽') {
        return true;
    }
    if chars
        .get(index..)
        .is_some_and(|suffix| suffix.starts_with(&['р', 'у', 'б']))
    {
        return true;
    }
    chars.get(index) == Some(&'р')
        && chars
            .get(index + 1)
            .is_none_or(|next| !next.is_alphanumeric())
}

fn validate_config(config: &FeatureConfig) -> Result<(), String> {
    if config.version != RULE_VERSION
        || config.max_messages <= 0
        || config.max_chat_age_seconds <= 0
    {
        return Err("version and freshness limits must be set".to_owned());
    }
    let mut feature_names = HashSet::new();
    for feature in &config.features {
        if feature.name.trim().is_empty()
            || feature.weight <= 0
            || (feature.markers.is_empty() && feature.matcher.is_none())
            || feature
                .matcher
                .as_deref()
                .is_some_and(|matcher| !matches!(matcher, "concrete_ruble_amount" | "large_number"))
            || !feature_names.insert(feature.name.as_str())
        {
            return Err(format!("invalid or duplicate feature: {}", feature.name));
        }
    }
    let mut rules = HashSet::new();
    for campaign in &config.campaigns {
        let mut references = campaign
            .require_all
            .iter()
            .chain(campaign.require_any.iter().flatten());
        if campaign.rule.trim().is_empty()
            || campaign.subtype.trim().is_empty()
            || campaign.reason.trim().is_empty()
            || campaign.minimum_score <= 0
            || !rules.insert(campaign.rule.as_str())
            || references.any(|feature| !feature_names.contains(feature.as_str()))
        {
            return Err(format!("invalid campaign definition: {}", campaign.rule));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRESH: (i64, Option<i64>) = (1, Some(60));

    fn classify_fresh(message: &str) -> Option<CampaignMatch> {
        classify(Some(message), FRESH.0, FRESH.1, false)
    }

    #[test]
    fn embedded_feature_configuration_is_valid_and_features_are_shared() {
        let parsed: FeatureConfig = toml::from_str(FEATURE_CONFIG).unwrap();
        validate_config(&parsed).unwrap();
        assert!(
            parsed
                .campaigns
                .iter()
                .all(|campaign| !campaign.auto_ban_eligible)
        );
        assert!(parsed.features.len() >= 8);
        assert!(parsed.campaigns.len() >= 3);
    }

    #[test]
    fn catches_manual_money_dm_examples_through_feature_combinations() {
        for message in [
            "Нужны срочно деньги???Пиши-мне!!!",
            "Нуждаешься в ДEHbГAX ? Маякни в личные, по быстрому решим этот момент!",
            "Думаешь где взять деньжат? Маякни в личные, по быстрому решим этот момент!",
            "В поиске ДEHEГ ? Есть вариации как для девушек так и для мужчин, в личные!",
            "Даю деньги за улыбку пиши",
            "Дополнительная прибыль в свободное время. Детали обсудим",
            "Вакансии с хорошим доходом! 14+ Зп от 75$ за смену. Все детали у @contact",
            "Есть деньги за услугу — пишите в личные сообщения",
            "ДОХОД Д0 2ОК В ДЕНЬ! ПИШИ СЮДА https://t.me/m/example",
            "Помогу с деньгами за услугу — пишите",
            "Ищем людей на вечер, без опыта, от 8500 на руки. За подробностями пиши в личку",
        ] {
            assert_eq!(
                classify_fresh(message).map(|campaign| campaign.kind),
                Some(CampaignKind::MoneyDmFunnel),
                "missed: {message}"
            );
        }
    }

    #[test]
    fn specific_campaigns_take_precedence_over_the_general_money_contact_rule() {
        let paid_task =
            classify_fresh("Есть дельце на пару часов, по завершению 7.000р!!!").unwrap();
        assert_eq!(paid_task.kind, CampaignKind::PaidShortTaskOffer);
        assert_eq!(paid_task.rule, "fresh_paid_short_task_with_concrete_payout");

        let product = classify_fresh("Предлагаю товар на пробу, мест всего 7").unwrap();
        assert_eq!(product.kind, CampaignKind::LimitedProductTrial);
        assert_eq!(product.rule, "fresh_limited_product_trial_offer");
    }

    #[test]
    fn catches_generalized_work_and_compensation_combinations() {
        for (message, expected_kind) in [
            (
                "Кто завтра свободен для комплектования материалов к встрече? 5 600 на руки + обеды и компенсация дороги",
                CampaignKind::PaidWorkOpportunity,
            ),
            (
                "Помощь в ресторане. Зп от 55$ за смену. По всем вопросам к нему - @contact",
                CampaignKind::MoneyDmFunnel,
            ),
            (
                "Курьер в шоп. Плата в день 200$. Сегодня аванс. Без вложений. В лс @someone",
                CampaignKind::MoneyDmFunnel,
            ),
            (
                "Несколько часов твоего времени и 9 ОООр твои. Обращайся",
                CampaignKind::MoneyDmFunnel,
            ),
            (
                "Есть несколько свободных мест на вечер. Без опыта, от 8500 на руки. За выполнение предусмотрено. Напиши в личные сообщения",
                CampaignKind::MoneyDmFunnel,
            ),
            (
                "Дам 1800 за небольшую помощь",
                CampaignKind::PaidShortTaskOffer,
            ),
            (
                "Нужно три человека в криптопроект. Открыты к новичкам. Пишите: @someone",
                CampaignKind::MoneyDmFunnel,
            ),
            (
                "Мы в поисках сотрудников. От 15,ОООр. Начинай уже сегодня! С 16+ лет. Пиши по контакту: @someone",
                CampaignKind::PaidWorkOpportunity,
            ),
            (
                "Дам 5000 за пару лёгких движений, жду @someone",
                CampaignKind::MoneyDmFunnel,
            ),
            ("ДАМ В ДОЛГ!!!", CampaignKind::FinancialOffer),
            (
                "Кто в поиске ДEHEГ? Маякни в личные, по быстрому решим этот вопрос!",
                CampaignKind::MoneyDmFunnel,
            ),
        ] {
            assert_eq!(
                classify_fresh(message).map(|campaign| campaign.kind),
                Some(expected_kind),
                "missed: {message}"
            );
        }
    }

    #[test]
    fn catches_repeated_free_audio_and_book_contact_campaigns() {
        for message in [
            "До жути понравилась книга «Время — деньги». Есть и в аудио, могу переслать, я сам и читал, и слушал.",
            "Кому интересно, могу скинуть подкаст о репутации и навыках — он действительно полезный",
        ] {
            let campaign = classify_fresh(message).unwrap();
            assert_eq!(campaign.kind, CampaignKind::ContentShareFunnel);
            assert_eq!(campaign.rule, "fresh_content_share_funnel");
        }
    }

    #[test]
    fn ordinary_money_discussion_without_offer_and_contact_stays_unmatched() {
        for message in [
            "В статье обсуждают доходы и расходы семьи",
            "Ищу работу, но пока ничего не нашёл",
            "Нужны деньги на ремонт, кто знает хорошего мастера?",
            "Нашёл книгу «Время — деньги», интересное обсуждение",
            "Вакансии сейчас часто обсуждают в новостях",
            "Пять лет прироста дохода у компании обсуждали в статье",
            "Я не дам в долг, сначала надо понять, что произошло",
        ] {
            assert!(
                classify_fresh(message).is_none(),
                "false positive: {message}"
            );
        }
    }

    #[test]
    fn each_configured_first_message_campaign_emits_a_valid_assessment() {
        for message in [
            "Нужны срочно деньги???Пиши-мне!!!",
            "Есть дельце на пару часов, по завершению 7.000р!!!",
            "Кто завтра свободен для комплектования материалов? 5 600 на руки + компенсация дороги",
            "ДАМ В ДОЛГ!!!",
            "Предлагаю товар на пробу, мест всего 7",
            "До жути понравилась книга «Время — деньги». Есть и в аудио, могу переслать.",
        ] {
            let campaign = classify_fresh(message).unwrap();
            let serialized = serde_json::to_string(&campaign.assessment(true)).unwrap();
            teloxide_antispam::assessment::NewUserAuditAssessment::parse_for_modalities(
                &serialized,
                false,
                true,
            )
            .unwrap_or_else(|error| panic!("invalid assessment for {message:?}: {error}"));
        }
    }

    #[test]
    fn campaign_requires_fresh_low_activity_account() {
        let message = "Нужны срочно деньги???Пиши-мне!!!";
        assert!(classify(Some(message), 11, Some(60), false).is_none());
        assert!(classify(Some(message), 2, Some(259_201), false).is_none());
        assert!(classify(Some(message), 2, None, false).is_none());
    }

    #[test]
    fn adult_personal_channel_rule_is_stable() {
        assert_eq!(
            classify(None, 1, Some(60), true).map(|campaign| campaign.kind),
            Some(CampaignKind::AdultPersonalChannel)
        );
        assert!(CampaignMatch::from_rule("fresh_adult_personal_channel").is_some());
    }

    #[test]
    fn mixed_script_normalization_handles_latin_transliteration_and_soft_sign_evasion() {
        let normalized = normalize_mixed_script_homoglyphs("ДEHEГ");
        assert!(
            normalized.to_lowercase().contains("денег"),
            "normalized as {normalized:?}"
        );
        assert!(
            normalize_mixed_script_homoglyphs("ДEHbГAX")
                .to_lowercase()
                .contains("деньгах")
        );
        assert_eq!(normalize_mixed_script_homoglyphs("Hello123"), "Hello123");
    }

    #[test]
    fn deterministic_assessment_and_signal_preserve_rule_evidence() {
        let campaign = classify_fresh("Нужны срочно деньги???Пиши-мне!!!").unwrap();
        let assessment = campaign.assessment(true);
        assert_eq!(
            assessment["first_message_assessment"]["direct_dm_offer"],
            false
        );
        assert_eq!(
            campaign.score_signal()["feature_config_version"],
            "new-user-features-v2"
        );
        assert_eq!(
            campaign.score_signal()["label"],
            "deterministic_campaign_review_candidate"
        );
        assert_eq!(campaign.score_signal()["auto_ban_eligible"], false);
        assert!(campaign.feature_score >= 65);
        assert!(campaign.matched_features.contains(&"economic".to_owned()));
    }

    #[test]
    fn plausible_book_and_audio_offers_are_review_only_candidates() {
        for message in [
            "До жути понравилась книга «Время — деньги». Есть и в аудио, могу переслать, я сам и читал, и слушал.",
            "Кому интересно, могу скинуть подкаст о репутации и навыках — он действительно полезный",
        ] {
            let campaign = classify_fresh(message).unwrap();
            assert_eq!(campaign.kind, CampaignKind::ContentShareFunnel);
            assert!(!campaign.auto_ban_eligible);
            assert_eq!(
                campaign.score_signal()["label"],
                "deterministic_campaign_review_candidate"
            );
            let assessment = campaign.assessment(true);
            assert_eq!(assessment["first_message_assessment"]["confidence"], 0.4);
            assert_eq!(
                assessment["first_message_assessment"]["direct_dm_offer"],
                false
            );
            assert_eq!(
                assessment["first_message_assessment"]["evidence"],
                serde_json::json!([])
            );
        }
    }

    #[test]
    fn stored_verdict_survives_rule_removal_and_configuration_changes() {
        let original = classify_fresh("Нужны срочно деньги???Пиши-мне!!!").unwrap();
        let evidence = original.score_signal();
        let restored =
            CampaignMatch::from_stored_evidence("removed_from_config", Some(&evidence)).unwrap();

        assert_eq!(restored.kind, original.kind);
        assert_eq!(restored.rule, original.rule);
        assert_eq!(restored.subtype, original.subtype);
        assert_eq!(restored.reason, original.reason);
        assert_eq!(restored.rule_version, original.rule_version);
        assert_eq!(
            restored.feature_config_version,
            original.feature_config_version
        );
        assert_eq!(restored.auto_ban_eligible, original.auto_ban_eligible);
        assert_eq!(restored.matched_features, original.matched_features);
    }
}
