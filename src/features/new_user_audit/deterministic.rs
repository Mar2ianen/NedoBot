use serde_json::{Value, json};
use teloxide_antispam::preprocess::prepare_text;

pub(crate) const RULE_VERSION: &str = "new-user-campaign-v1";

const MAX_MESSAGES: i64 = 3;
const MAX_CHAT_AGE_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CampaignKind {
    MoneyDmFunnel,
    LimitedProductTrial,
    AdultPersonalChannel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CampaignMatch {
    pub kind: CampaignKind,
    pub rule: &'static str,
    pub subtype: &'static str,
    pub reason: &'static str,
}

impl CampaignMatch {
    pub fn assessment(self, has_first_message: bool) -> Value {
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
                    {"marker": "direct_messages", "quote": "Сочетание явного призыва перейти в личные сообщения с предложением денег или работы."},
                    {"marker": "self_help_or_finance_promo", "quote": "Денежное или рабочее предложение новому участнику."}
                ],
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
                "evidence": [
                    {"marker": "send_or_share_offer", "quote": "Предложение товара на пробу ограничено числом мест."}
                ],
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
                "summary": "Текст сообщения не использовался: детерминированное правило обнаружило рекламный adult-канал.",
                "confidence": 1.0
            })
        };

        let evidence_source = match self.kind {
            CampaignKind::MoneyDmFunnel => "first_message",
            CampaignKind::LimitedProductTrial => "first_message",
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

    pub fn score_signal(self) -> Value {
        json!({
            "class": "deterministic_campaign",
            "label": "deterministic_campaign_fast_path",
            "subtype": self.subtype,
            "reason": self.reason,
            "rule": self.rule,
            "rule_version": RULE_VERSION,
            "decision": "auto_spam",
            "warning_strength": "strong"
        })
    }
}

pub(crate) fn classify(
    first_message: Option<&str>,
    message_count: i64,
    chat_age_seconds: Option<i64>,
    personal_channel_has_adult_links: bool,
) -> Option<CampaignMatch> {
    let chat_age_seconds = chat_age_seconds?;
    if message_count > MAX_MESSAGES || !(0..=MAX_CHAT_AGE_SECONDS).contains(&chat_age_seconds) {
        return None;
    }

    if personal_channel_has_adult_links {
        return Some(CampaignMatch {
            kind: CampaignKind::AdultPersonalChannel,
            rule: "fresh_adult_personal_channel",
            subtype: "adult_personal_channel_promo",
            reason: "У свежего участника личный канал содержит ссылки на adult-промо.",
        });
    }

    let normalized = prepare_text(first_message?).model_text();
    let limited_product_trial = normalized.contains("на пробу")
        && normalized.contains("товар")
        && [
            "мест всего",
            "мест осталось",
            "осталось мест",
            "ограниченное количество",
        ]
        .iter()
        .any(|marker| normalized.contains(marker))
        && ["дам", "предлагаю", "отдам"]
            .iter()
            .any(|marker| normalized.contains(marker));
    if limited_product_trial {
        return Some(CampaignMatch {
            kind: CampaignKind::LimitedProductTrial,
            rule: "fresh_limited_product_trial_offer",
            subtype: "product_trial_bait",
            reason: "Свежий участник рекламирует товар на пробу с искусственным ограничением числа мест.",
        });
    }

    let money_or_work_offer = [
        "деньг",
        "деньжат",
        "денег",
        "заработ",
        "доход",
        "выплат",
        "руб",
        "₽",
        "$",
        "доллар",
        "евро",
        "зарплат",
        "зп ",
        "аванс",
        "оплат",
        "за пару часов",
        "за смен",
    ]
    .iter()
    .any(|marker| normalized.contains(marker));
    let explicit_offer = [
        "получ",
        "с меня",
        "дам ",
        "плачу",
        "оплат",
        "аванс",
        "без опыта",
        "требуются работник",
        "ищем помощник",
        "помощь в",
        "маякни",
        "решим этот вопрос",
        "решим этот момент",
        "кидай +",
        "кидайте +",
        "заработ",
        "ваканси",
        "работ",
    ]
    .iter()
    .any(|marker| normalized.contains(marker));
    let private_contact_cta = [
        "в личк",
        "личные сообщения",
        "в директ",
        "директ",
        "в лс",
        "маякни",
        "маякните",
        "черканите",
        "пиши",
        "пишите",
        "напиши",
        "напишите",
        "кидай +",
        "кидайте +",
        "@",
    ]
    .iter()
    .any(|marker| normalized.contains(marker));

    (money_or_work_offer && explicit_offer && private_contact_cta).then_some(CampaignMatch {
        kind: CampaignKind::MoneyDmFunnel,
        rule: "fresh_money_work_private_contact",
        subtype: "money_dm_bait",
        reason: "Свежий участник предлагает деньги или работу и сразу переводит контакт в личные сообщения.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRESH_LOW_ACTIVITY: (i64, Option<i64>) = (1, Some(60));

    #[test]
    fn catches_money_dm_templates_and_mixed_script_evasion() {
        for message in [
            "Думаешь где взять деньжат? Маякни в личные, по быстрому решим этот момент!",
            "Думаешь где взять ДEHEГ? Маякни в личные, по быстрому решим этот вопрос!",
            "👀 Есть желающие получить 500 ₽? Черканите мне в личку 💰",
            "💵 Кидай + в директ — с меня 400 ₽",
            "Помощь в ресторане. Зп от 55$ за смену. По всем вопросам к нему - @GoodWorkings",
            "Требуются работники. Оплата от 5 000 рублей. Пишите в личные сообщения.",
            "Есть возможность! От 9.000 за пару часов! Без опыта + авансы. Пиши -- @DMX_2213",
            "Дам0.5 на пробу любого товара мест всего 7",
        ] {
            assert_eq!(
                classify(
                    Some(message),
                    FRESH_LOW_ACTIVITY.0,
                    FRESH_LOW_ACTIVITY.1,
                    false
                )
                .map(|campaign| campaign.kind),
                Some(if message.contains("на пробу") {
                    CampaignKind::LimitedProductTrial
                } else {
                    CampaignKind::MoneyDmFunnel
                }),
                "missed: {message}"
            );
        }
    }

    #[test]
    fn catches_adult_channel_for_fresh_low_activity_member_without_text_evidence() {
        assert_eq!(
            classify(None, 1, Some(60), true).map(|campaign| campaign.kind),
            Some(CampaignKind::AdultPersonalChannel)
        );
    }

    #[test]
    fn does_not_flag_topic_messages_or_ambiguous_profile_signals() {
        for message in [
            "Они изобрели пасту ГОИ?",
            "Отлично она 4к тянет",
            "На одной 3060 и 64 GB производительность та же",
            "Кухни от производителя, скидка 15%, пришлём каталог по запросу @MasterMKuhni",
            "Маякни в личные, могу прислать ссылку на документацию",
        ] {
            assert!(
                classify(
                    Some(message),
                    FRESH_LOW_ACTIVITY.0,
                    FRESH_LOW_ACTIVITY.1,
                    false
                )
                .is_none(),
                "unexpected match: {message}"
            );
        }
    }

    #[test]
    fn requires_fresh_low_activity_context() {
        let message = "Думаешь где взять деньжат? Маякни в личные, по быстрому решим вопрос!";
        assert!(classify(Some(message), 4, Some(60), false).is_none());
        assert!(classify(Some(message), 1, Some(90_000), false).is_none());
        assert!(classify(Some(message), 1, None, false).is_none());
    }

    #[test]
    fn fast_path_assessment_satisfies_the_strict_audit_schema() {
        let campaign = classify(
            Some("Получить 500 рублей? Пишите в личку."),
            1,
            Some(60),
            false,
        )
        .unwrap();
        let serialized = serde_json::to_string(&campaign.assessment(true)).unwrap();
        let parsed = teloxide_antispam::assessment::NewUserAuditAssessment::parse_for_modalities(
            &serialized,
            false,
            true,
        )
        .unwrap();
        assert!(parsed.first_message_assessment.unwrap().direct_dm_offer);

        let product_trial = classify(
            Some("Дам0.5 на пробу любого товара мест всего 7"),
            1,
            Some(60),
            false,
        )
        .unwrap();
        let serialized = serde_json::to_string(&product_trial.assessment(true)).unwrap();
        let parsed = teloxide_antispam::assessment::NewUserAuditAssessment::parse_for_modalities(
            &serialized,
            false,
            true,
        )
        .unwrap();
        assert!(parsed.first_message_assessment.unwrap().offtopic_promo);
    }
}
