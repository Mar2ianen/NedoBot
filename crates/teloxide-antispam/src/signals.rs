//! Чистое скоринг-ядро baseline-аудита новых пользователей.
//!
//! Сигналы риска, decision tree и 4PL-модель telegram-id. Без SQL,
//! Telegram и LLM: бот загружает snapshots, маппит строки на
//! [`NewUserFeatures`] и вызывает [`analyze_risk`].

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::text::{has_mixed_script_homoglyphs, normalize_cyrillic_homoglyphs};

/// Калиброванная 4PL-модель спам-вероятности telegram-id: монотонная сигмоида
/// по величине id. Живёт в крейне, чтобы скоринг был переиспользуем между
/// инстансами; TOML-десериализация через реэкспорт в `config_file`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramIdRiskModel {
    pub floor: f64,
    pub ceil: f64,
    pub k: f64,
    pub midpoint_billion: f64,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct NewUserAnalysisConfig {
    pub recent_id_ratio_threshold: f64,
    pub old_user_message_threshold: i64,
    pub review_threshold: i32,
    pub risk_profile: String,
    pub risk_profile_version: String,
    pub telegram_id_model_version: Option<String>,
    pub telegram_id_model: Option<TelegramIdRiskModel>,
}
impl Default for NewUserAnalysisConfig {
    fn default() -> Self {
        Self {
            recent_id_ratio_threshold: 0.92,
            old_user_message_threshold: 5,
            review_threshold: 70,
            risk_profile: "legacy".to_string(),
            risk_profile_version: "legacy".to_string(),
            telegram_id_model_version: None,
            telegram_id_model: None,
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct NewUserFeatures {
    pub chat_id: i64,
    pub telegram_user_id: i64,
    pub first_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    pub account_seen_age_sec: Option<i64>,
    pub chat_age_sec: Option<i64>,
    pub first_message_id: Option<i32>,
    pub last_message_id: Option<i32>,
    pub message_count: i64,
    pub reply_count: i64,
    pub link_count: i64,
    pub media_count: i64,
    pub voice_count: i64,
    pub reply_to_channel_post_count: i64,
    pub reply_to_bot_count: i64,
    pub top_level_message_count: i64,
    pub reply_to_comment_count: i64,
    pub message_count_24h: i64,
    pub link_count_24h: i64,
    pub burst_messages_per_min: Option<f64>,
    pub first_message_text: Option<String>,
    pub first_message_reply_context: Option<String>,
    pub last_message_text: Option<String>,
    pub recent_message_texts: Vec<String>,
    pub text_texture: TextTexture,
    pub message_style: MessageStyle,
    pub id_rank_ratio: Option<f64>,
    pub username: Option<String>,
    pub username_reuse_count: i64,
    pub username_reuse_spammer_count: i64,
    pub shared_spammer_identity: bool,
    pub lols_spammer_identity: bool,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub display_name: Option<String>,
    pub display_name_reuse_count: i64,
    pub display_name_reuse_spammer_count: i64,
    pub is_bot: bool,
    pub is_premium: Option<bool>,
    pub language_code: Option<String>,
    pub bio: Option<String>,
    pub profile_photo_file_id: Option<String>,
    pub profile_photo_file_unique_id: Option<String>,
    pub profile_photo_count: Option<i32>,
    pub profile_photo_reuse_count: i64,
    pub profile_photo_width: Option<i32>,
    pub profile_photo_height: Option<i32>,
    pub emoji_status_custom_emoji_id: Option<String>,
    pub profile_accent_color_id: Option<i16>,
    pub personal_channel_chat_id: Option<i64>,
    pub personal_channel_title: Option<String>,
    pub personal_channel_title_reuse_count: i64,
    pub personal_channel_title_reuse_spammer_count: i64,
    pub identity_snapshot_count: i64,
    pub identity_display_name_count: i64,
    pub identity_username_count: i64,
    pub personal_channel_username: Option<String>,
    pub personal_channel_message_count: Option<i32>,
    pub personal_channel_last_message_id: Option<i32>,
    pub personal_channel_last_message_at: Option<chrono::DateTime<chrono::Utc>>,
    pub personal_channel_last_text: Option<String>,
    pub personal_channel_has_adult_links: bool,
    pub personal_channel_refreshed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub personal_channel_fetch_error: Option<String>,
    pub member_status: Option<String>,
    pub member_is_present: Option<bool>,
    pub member_is_admin: Option<bool>,
    pub join_event_seen: bool,
    pub invite_link: Option<String>,
    pub via_chat_folder_invite_link: bool,
}
#[derive(Debug, Clone, Default)]
pub struct TextTexture {
    pub normalized_count: i64,
    pub distinct_normalized_count: i64,
    pub duplicate_normalized_count: i64,
    pub max_reuse_count: i64,
    pub max_pairwise_similarity: Option<f64>,
    pub avg_message_len: Option<f64>,
    pub repetitive_pattern: bool,
}
#[derive(Debug, Clone, Default)]
pub struct MessageStyle {
    pub text_message_count: i64,
    pub single_exclamation_ending_count: i64,
    pub repeated_exclamation_ending_count: i64,
    pub period_ending_count: i64,
    pub emoji_message_count: i64,
    pub emoji_ending_count: i64,
    pub single_emoji_message_count: i64,
    pub single_emoji_ending_count: i64,
    pub adjacent_emoji_message_count: i64,
    pub other_non_text_ending_count: i64,
    pub unmatched_closing_parenthesis_ending_count: i64,
}
#[derive(Debug, Clone, Copy)]
pub enum MessageStylePersona {
    General,
    GenericFeminine,
    SuccessPersona,
}
impl MessageStylePersona {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::GenericFeminine => "generic_feminine",
            Self::SuccessPersona => "success_persona",
        }
    }
}
#[derive(Debug, Clone)]
pub struct RiskAnalysis {
    pub score: i32,
    pub level: String,
    pub primary_class: Option<String>,
    pub class_scores: Value,
    pub labels: Vec<String>,
    pub reasons: Vec<String>,
    pub signals: Value,
}
const SHARED_SPAM_DECISION_TREE_VERSION: &str = "shared-spam-tree-v5";
#[derive(Debug, Clone, Copy)]
struct SharedSpamTreeLeaf {
    class: SpamClass,
    label: &'static str,
    reason: &'static str,
    path: &'static [&'static str],
}
#[derive(Debug, Clone, Copy)]
pub enum WarningStrength {
    Weak,
    Supporting,
    Strong,
    Mitigating,
}
impl WarningStrength {
    fn as_str(self) -> &'static str {
        match self {
            Self::Weak => "weak",
            Self::Supporting => "supporting",
            Self::Strong => "strong",
            Self::Mitigating => "mitigating",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SpamClass {
    AdultPersonalChannel,
    ForeignInviteLink,
    KnownSpammer,
    LlmProfileBait,
    PromoDmBait,
    LinkDropper,
    FreshAccount,
}
impl SpamClass {
    pub fn all() -> [Self; 7] {
        [
            Self::AdultPersonalChannel,
            Self::ForeignInviteLink,
            Self::KnownSpammer,
            Self::LlmProfileBait,
            Self::PromoDmBait,
            Self::LinkDropper,
            Self::FreshAccount,
        ]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SpamClass::AdultPersonalChannel => "adult_personal_channel_promo",
            SpamClass::ForeignInviteLink => "foreign_invite_link_spam",
            SpamClass::KnownSpammer => "known_spammer_identity",
            SpamClass::LlmProfileBait => "llm_profile_bait",
            SpamClass::PromoDmBait => "promo_dm_bait",
            SpamClass::LinkDropper => "link_dropper",
            SpamClass::FreshAccount => "fresh_account_risk",
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct RiskSignal {
    pub class: SpamClass,
    pub coefficient: i32,
    pub label: &'static str,
    pub reason: &'static str,
}
impl RiskSignal {
    fn warning_strength(self) -> WarningStrength {
        match self.coefficient {
            ..=-1 => WarningStrength::Mitigating,
            0..=4 => WarningStrength::Weak,
            5..=14 => WarningStrength::Supporting,
            _ => WarningStrength::Strong,
        }
    }
}
#[derive(Debug, Default)]
struct RiskAccumulator {
    score: i32,
    class_scores: std::collections::BTreeMap<SpamClass, i32>,
    labels: Vec<String>,
    reasons: Vec<String>,
    signals: Vec<Value>,
}
impl RiskAccumulator {
    fn add(&mut self, signal: RiskSignal) {
        self.score += signal.coefficient;
        *self.class_scores.entry(signal.class).or_default() += signal.coefficient;
        self.labels.push(signal.label.to_string());
        self.reasons.push(signal.reason.to_string());
        self.signals.push(json!({
            "class": signal.class.as_str(),
            "label": signal.label,
            "warning_strength": signal.warning_strength().as_str(),
            "coefficient": signal.coefficient,
            "reason": signal.reason,
        }));
    }

    fn add_optional(&mut self, signal: Option<RiskSignal>) {
        if let Some(signal) = signal {
            self.add(signal);
        }
    }

    fn finish(mut self, review_threshold: i32) -> RiskAnalysis {
        self.score = self.score.min(100);
        let level = match self.score {
            score if score >= review_threshold => "high",
            40..=69 => "medium",
            _ => "low",
        }
        .to_string();
        let primary_class = self
            .class_scores
            .iter()
            .max_by_key(|(_, score)| *score)
            .map(|(class, _)| class.as_str().to_string());
        let class_scores = self
            .class_scores
            .iter()
            .fold(Map::new(), |mut acc, (class, score)| {
                acc.insert(class.as_str().to_string(), json!(score));
                acc
            });

        self.labels.sort();
        self.labels.dedup();

        RiskAnalysis {
            score: self.score,
            level,
            primary_class,
            class_scores: Value::Object(class_scores),
            labels: self.labels,
            reasons: self.reasons,
            signals: Value::Array(self.signals),
        }
    }
}
pub fn analyze_risk(
    features: &NewUserFeatures,
    config: &NewUserAnalysisConfig,
    is_old_active_user: bool,
) -> RiskAnalysis {
    match is_old_active_user {
        true => old_active_user_risk(),
        false => analyze_new_or_low_activity_user(features, config),
    }
}
fn old_active_user_risk() -> RiskAnalysis {
    RiskAnalysis {
        score: 0,
        level: "low".to_string(),
        primary_class: None,
        class_scores: json!({}),
        labels: vec!["old_active_user".to_string()],
        reasons: vec![
            "Existing active chat participant; profile audit kept for baseline only.".to_string(),
        ],
        signals: json!([]),
    }
}
pub fn analyze_new_or_low_activity_user(
    features: &NewUserFeatures,
    config: &NewUserAnalysisConfig,
) -> RiskAnalysis {
    let username_stats = username_stats(features.username.as_deref());
    let mut risk = RiskAccumulator::default();

    risk.add_optional(shared_spammer_signal(features.shared_spammer_identity));
    risk.add_optional(lols_spammer_signal(features.lols_spammer_identity));
    risk.add_optional(message_count_signal(features));
    risk.add_optional(link_signal(features));
    risk.add_optional(foreign_invite_link_signal(features));
    risk.add_optional(recent_id_signal(features, config));
    risk.add_optional(username_signal(features, &username_stats));
    risk.add_optional(display_name_signal(features));
    risk.add_optional(identity_rotation_signal(features));
    risk.add_optional(profile_photo_signal(features));
    risk.add_optional(feminine_name_signal(features));
    risk.add_optional(homoglyph_profile_signal(features));
    risk.add_optional(message_texture_signal(features));
    risk.add_optional(chat_position_signal(features));
    for signal in message_style_signals(features) {
        risk.add(signal);
    }
    risk.add_optional(reply_to_comment_mitigation_signal(features));
    risk.add_optional(chat_age_signal(features));

    for signal in personal_channel_signals(features) {
        risk.add(signal);
    }

    risk.add_optional(short_bio_signal(features));
    risk.add_optional(explicit_adult_bio_signal(features));
    risk.add_optional(profile_bio_subscription_offer_signal(features));
    risk.add_optional(member_status_signal(features));
    let mut analysis = risk.finish(config.review_threshold);
    if let Some(leaf) = shared_spam_decision_tree(features, config) {
        apply_shared_spam_tree_leaf(&mut analysis, leaf, config.review_threshold);
    }
    analysis
}
fn shared_spam_decision_tree(
    features: &NewUserFeatures,
    config: &NewUserAnalysisConfig,
) -> Option<SharedSpamTreeLeaf> {
    let has_personal_channel = features.personal_channel_chat_id.is_some();
    let only_channel_post_comments = only_channel_post_comments(features);
    let has_recent_id = features
        .id_rank_ratio
        .is_some_and(|ratio| ratio >= config.recent_id_ratio_threshold);
    let has_random_username = username_stats(features.username.as_deref()).has_random_suffix;

    if has_personal_channel && features.personal_channel_has_adult_links {
        return Some(SharedSpamTreeLeaf {
            class: SpamClass::AdultPersonalChannel,
            label: "tree_personal_channel_adult_funnel",
            reason: "Attached personal channel contains adult-promotion links",
            path: &[
                "personal_channel_attached",
                "personal_channel_has_adult_links",
            ],
        });
    }

    if has_personal_channel && personal_channel_has_invite_link(features) {
        return Some(SharedSpamTreeLeaf {
            class: SpamClass::LinkDropper,
            label: "tree_personal_channel_invite_funnel",
            reason: "Attached personal channel contains a Telegram invite funnel",
            path: &[
                "personal_channel_attached",
                "personal_channel_has_invite_link",
            ],
        });
    }

    let is_fresh_in_chat = features.chat_age_sec.is_some_and(|age| age < 6 * 60 * 60);
    let is_recent_low_activity =
        features.chat_age_sec.is_some_and(|age| age < 24 * 60 * 60) && features.message_count <= 3;
    if is_fresh_in_chat
        && features.message_count <= 2
        && features
            .first_message_text
            .as_deref()
            .is_some_and(is_fresh_contact_send_offer)
    {
        return Some(SharedSpamTreeLeaf {
            class: SpamClass::PromoDmBait,
            label: "tree_fresh_contact_send_offer",
            reason: "A just-arrived low-activity user combines a direct-contact call to action with a promise to send something",
            path: &[
                "chat_age_under_six_hours",
                "one_or_two_messages",
                "direct_contact_call_to_action",
                "promise_to_send_content",
            ],
        });
    }

    if is_recent_low_activity
        && features.first_message_text.as_deref().is_some_and(|text| {
            is_fresh_paid_task_offer(text) && (is_fresh_in_chat || has_contact_call_to_action(text))
        })
    {
        return Some(SharedSpamTreeLeaf {
            class: SpamClass::PromoDmBait,
            label: "tree_fresh_paid_task_offer",
            reason: "A just-arrived low-activity user names a concrete payment amount for a small or easy task",
            path: &[
                "chat_age_under_twenty_four_hours",
                "up_to_three_messages",
                "explicit_payment_amount",
                "small_or_easy_task",
            ],
        });
    }

    if is_recent_low_activity
        && features
            .first_message_text
            .as_deref()
            .is_some_and(is_fresh_money_work_promotion)
    {
        return Some(SharedSpamTreeLeaf {
            class: SpamClass::PromoDmBait,
            label: "tree_fresh_money_work_contact_funnel",
            reason: "A just-arrived low-activity user combines a money/work offer with a direct-contact call to action",
            path: &[
                "chat_age_under_twenty_four_hours",
                "up_to_three_messages",
                "money_or_work_offer",
                "direct_contact_call_to_action",
            ],
        });
    }

    if features.chat_age_sec.is_some_and(|age| age < 24 * 60 * 60)
        && features.message_count <= 3
        && has_recent_id
        && features.text_texture.duplicate_normalized_count > 0
    {
        return Some(SharedSpamTreeLeaf {
            class: SpamClass::FreshAccount,
            label: "tree_fresh_recent_id_repeated_message",
            reason: "A recent-ID low-activity user repeats a normalized message shortly after joining",
            path: &[
                "chat_age_under_twenty_four_hours",
                "up_to_three_messages",
                "recent_telegram_id",
                "repeated_normalized_message",
            ],
        });
    }

    if has_personal_channel
        && personal_channel_has_external_link(features)
        && (is_fresh_in_chat || only_channel_post_comments)
    {
        return Some(SharedSpamTreeLeaf {
            class: SpamClass::LinkDropper,
            label: "tree_fresh_channel_external_link",
            reason: "A fresh or channel-post-only participant routes traffic through an attached channel link",
            path: &[
                "personal_channel_attached",
                "personal_channel_has_external_link",
                "fresh_or_only_channel_post_comments",
            ],
        });
    }

    if only_channel_post_comments && has_recent_id {
        return Some(SharedSpamTreeLeaf {
            class: SpamClass::FreshAccount,
            label: "tree_channel_comments_with_recent_id",
            reason: "A recent Telegram ID only replies directly to channel posts",
            path: &["only_channel_post_comments", "recent_telegram_id"],
        });
    }

    if has_recent_id && has_random_username {
        return Some(SharedSpamTreeLeaf {
            class: SpamClass::FreshAccount,
            label: "tree_recent_id_random_username",
            reason: "A recent Telegram ID is paired with a random-suffix username",
            path: &["recent_telegram_id", "username_random_suffix"],
        });
    }

    None
}
fn is_fresh_money_work_promotion(message: &str) -> bool {
    let normalized = normalize_cyrillic_homoglyphs(message).to_lowercase();
    if !has_contact_call_to_action(&normalized) {
        return false;
    }

    let has_work_or_income_offer = [
        "заработ",
        "подработ",
        "доход",
        "удаленк",
        "удалёнк",
        "ваканси",
        "оплата",
        "оплатим",
        "оплат",
        "без опыта",
        "простые задач",
        "простых задач",
        "обучаем с нуля",
        "разберется каждый",
        "разберётся каждый",
        "нужно три человека",
        "нужно 3 человека",
        "нужны три человека",
        "нужны 3 человека",
        "криптопроект",
        "подойдет любому",
        "подойдёт любому",
        "в день",
        "за пару часов",
        "без вложений",
    ]
    .iter()
    .any(|marker| normalized.contains(marker));
    let has_crypto_topic = ["крипт", "биткоин", "bitcoin", "btc", "трейдинг"]
        .iter()
        .any(|marker| normalized.contains(marker));
    let has_generic_work_offer = [
        "работ",
        "ваканси",
        "заработ",
        "подработ",
        "доход",
        "оплата",
        "оплат",
        "опыт не нужен",
        "простые задач",
        "подойдет любому",
        "подойдёт любому",
        "в день",
        "за пару часов",
        "без вложений",
    ]
    .iter()
    .any(|marker| normalized.contains(marker));
    has_work_or_income_offer && (has_crypto_topic || has_generic_work_offer)
}
fn is_fresh_paid_task_offer(message: &str) -> bool {
    let normalized = normalize_cyrillic_homoglyphs(message).to_lowercase();
    let has_explicit_amount = ["дам ", "отдам ", "плачу ", "оплата ", "оплачу "]
        .iter()
        .any(|marker| {
            normalized.match_indices(marker).any(|(offset, _)| {
                normalized[offset + marker.len()..]
                    .trim_start()
                    .chars()
                    .take(5)
                    .any(|character| character.is_ascii_digit())
            })
        })
        || (normalized
            .chars()
            .any(|character| character.is_ascii_digit())
            && ["руб", "₽", "тыс", "к за", "k за", "р в день"]
                .iter()
                .any(|marker| normalized.contains(marker)));
    let has_easy_task_offer = [
        "помощ",
        "задач",
        "движ",
        "за пару",
        "за смен",
        "за час",
        "несложн",
        "небольш",
        "легк",
        "лёгк",
    ]
    .iter()
    .any(|marker| normalized.contains(marker));

    has_explicit_amount && has_easy_task_offer
}
fn is_fresh_contact_send_offer(message: &str) -> bool {
    let normalized = normalize_cyrillic_homoglyphs(message).to_lowercase();
    has_contact_call_to_action(&normalized)
        && [
            "в лс",
            "лс",
            "личк",
            "личные сообщения",
            "в директ",
            "директ",
        ]
        .iter()
        .any(|marker| normalized.contains(marker))
        && [
            "отправлю",
            "пришлю",
            "скину",
            "перешлю",
            "сброшу",
            "поделюсь",
            "вышлю",
        ]
        .iter()
        .any(|marker| normalized.contains(marker))
}
fn has_contact_call_to_action(normalized: &str) -> bool {
    [
        "пиши",
        "напиши",
        "пишите",
        "напишите",
        "жду",
        "в лс",
        "лс",
        "личк",
        "личные сообщения",
        "свяжись",
        "свяжитесь",
        "@",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}
fn apply_shared_spam_tree_leaf(
    analysis: &mut RiskAnalysis,
    leaf: SharedSpamTreeLeaf,
    review_threshold: i32,
) {
    analysis.score = analysis.score.max(review_threshold.clamp(0, 100)).min(100);
    analysis.level = "high".to_string();
    analysis.primary_class = Some(leaf.class.as_str().to_string());
    analysis.labels.push(leaf.label.to_string());
    analysis.labels.sort();
    analysis.labels.dedup();
    analysis.reasons.push(leaf.reason.to_string());
    if let Some(signals) = analysis.signals.as_array_mut() {
        signals.push(json!({
            "class": leaf.class.as_str(),
            "label": leaf.label,
            "warning_strength": "strong",
            "decision_tree_version": SHARED_SPAM_DECISION_TREE_VERSION,
            "decision_tree_path": leaf.path,
            "decision": "manual_review",
            "reason": leaf.reason,
        }));
    }
}
fn message_count_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    match (features.message_count, features.message_count_24h) {
        (count, _) if count <= 1 => Some(RiskSignal {
            class: SpamClass::FreshAccount,
            coefficient: 12,
            label: "single_message_account",
            reason: "Only one observed chat message",
        }),
        (2..=3, count_24h) if count_24h >= features.message_count => Some(RiskSignal {
            class: SpamClass::FreshAccount,
            coefficient: 10,
            label: "short_burst_account",
            reason: "Few messages concentrated in a short window",
        }),
        _ => None,
    }
}
fn shared_spammer_signal(is_shared_spammer: bool) -> Option<RiskSignal> {
    is_shared_spammer.then_some(RiskSignal {
        class: SpamClass::KnownSpammer,
        coefficient: 70,
        label: "shared_spammer_identity",
        reason: "Telegram user id was confirmed as spam in another bot instance",
    })
}

/// Внешний LOLS banlist (локальное почасовое зеркало): покрытие нашей формы
/// спама 11/11 против 0/8 у CAS, поэтому вес выше слабого внешнего сигнала,
/// но ниже собственной подтверждённой репутации.
fn lols_spammer_signal(is_lols_spammer: bool) -> Option<RiskSignal> {
    is_lols_spammer.then_some(RiskSignal {
        class: SpamClass::KnownSpammer,
        coefficient: 50,
        label: "lols_spammer_identity",
        reason: "Telegram user id is present in the LOLS spammer banlist mirror",
    })
}
fn link_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    match features.link_count > 0 || features.link_count_24h > 0 {
        true => Some(RiskSignal {
            class: SpamClass::LinkDropper,
            coefficient: 18,
            label: "chat_message_has_link",
            reason: "New user posted a link",
        }),
        false => None,
    }
}
fn foreign_invite_link_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    let text = user_message_text_blob(features).to_lowercase();
    match (
        text.contains("t.me/+") || text.contains("telegram.me/+"),
        contains_cjk(&text),
        features.message_count,
    ) {
        (true, true, count) if count <= 5 => Some(RiskSignal {
            class: SpamClass::ForeignInviteLink,
            coefficient: 55,
            label: "foreign_invite_link_message",
            reason: "New user posted a Telegram invite link with foreign/CJK text",
        }),
        (true, false, count) if count <= 3 => Some(RiskSignal {
            class: SpamClass::ForeignInviteLink,
            coefficient: 32,
            label: "invite_link_from_new_user",
            reason: "Very new user posted a Telegram invite link",
        }),
        _ => None,
    }
}
fn recent_id_signal(
    features: &NewUserFeatures,
    config: &NewUserAnalysisConfig,
) -> Option<RiskSignal> {
    if let Some(model) = config.telegram_id_model.as_ref() {
        let probability = telegram_id_spam_probability(features.telegram_user_id, model);
        let coefficient = telegram_id_risk_coefficient(probability);
        return (coefficient > 0).then_some(RiskSignal {
            class: SpamClass::FreshAccount,
            coefficient,
            label: "telegram_id_spam_probability",
            reason: "Configured 4PL model assigns spam probability to the Telegram user id",
        });
    }

    features
        .id_rank_ratio
        .is_some_and(|ratio| ratio >= config.recent_id_ratio_threshold)
        .then_some(RiskSignal {
            class: SpamClass::FreshAccount,
            coefficient: 15,
            label: "recent_high_telegram_id",
            reason: "Telegram user id is in the recent high-id range observed by the bot",
        })
}
const TELEGRAM_ID_PROBABILITY_SIGNAL_FLOOR: f64 = 0.10;
const TELEGRAM_ID_PROBABILITY_SIGNAL_CEILING: f64 = 0.85;
const TELEGRAM_ID_MAX_RISK_COEFFICIENT: f64 = 15.0;
pub fn telegram_id_spam_probability(user_id: i64, model: &TelegramIdRiskModel) -> f64 {
    let id_billion = (user_id.max(0) as f64) / 1_000_000_000.0;
    let exponent = (model.k * (id_billion - model.midpoint_billion)).clamp(-60.0, 60.0);
    let sigmoid = 1.0 / (1.0 + (-exponent).exp());
    model.floor + (model.ceil - model.floor) * sigmoid
}
pub fn telegram_id_risk_coefficient(probability: f64) -> i32 {
    let normalized = ((probability - TELEGRAM_ID_PROBABILITY_SIGNAL_FLOOR)
        / (TELEGRAM_ID_PROBABILITY_SIGNAL_CEILING - TELEGRAM_ID_PROBABILITY_SIGNAL_FLOOR))
        .clamp(0.0, 1.0);
    (TELEGRAM_ID_MAX_RISK_COEFFICIENT * normalized).round() as i32
}
fn username_signal(features: &NewUserFeatures, stats: &UsernameStats) -> Option<RiskSignal> {
    if features.username_reuse_spammer_count > 0 {
        return Some(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 22,
            label: "username_reused_by_spammers",
            reason: "Username has already appeared on manually marked spammers",
        });
    }
    if features.username_reuse_count > 0 && features.message_count <= 3 {
        return Some(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 10,
            label: "username_reused_by_new_accounts",
            reason: "Username is reused by other seen accounts",
        });
    }
    match (stats.has_random_suffix, stats.has_digits, stats.digit_count) {
        (true, _, _) => Some(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 12,
            label: "username_random_suffix",
            reason: "Username has a bot-like/random suffix pattern",
        }),
        (false, true, 3..) => Some(RiskSignal {
            class: SpamClass::FreshAccount,
            coefficient: 5,
            label: "username_many_digits",
            reason: "Username contains many digits",
        }),
        _ => None,
    }
}
fn display_name_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    match (
        features.display_name_reuse_spammer_count,
        features.display_name_reuse_count,
        features.message_count,
    ) {
        (spammer_count, _, _) if spammer_count > 0 => Some(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 22,
            label: "display_name_reused_by_spammers",
            reason: "Display name has already appeared on manually marked spammers",
        }),
        (0, reuse_count, message_count) if reuse_count > 0 && message_count <= 3 => {
            Some(RiskSignal {
                class: SpamClass::LlmProfileBait,
                coefficient: 10,
                label: "display_name_reused_by_new_accounts",
                reason: "Display name is reused by other seen accounts",
            })
        }
        _ => None,
    }
}
/// Ротация identity: операторы меняют имена/юзернеймы между заходами
/// (LOLS-трекинг #namechange). Фото в счёт не идёт — аватарки меняют и живые.
fn identity_rotation_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    match (
        features.identity_display_name_count,
        features.identity_username_count,
    ) {
        (names, _) if names >= 2 => Some(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 12,
            label: "identity_display_name_rotation",
            reason: "User rotated public display names across profile refreshes",
        }),
        (_, usernames) if usernames >= 2 => Some(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 8,
            label: "identity_username_rotation",
            reason: "User rotated usernames across profile refreshes",
        }),
        _ => None,
    }
}

fn profile_photo_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    match has_profile_photo(features) {
        false => Some(RiskSignal {
            class: SpamClass::FreshAccount,
            coefficient: 6,
            label: "missing_profile_photo",
            reason: "No visible profile photo via Bot API",
        }),
        true => None,
    }
}
fn feminine_name_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    match (
        looks_like_feminine_first_name(features.first_name.as_deref()),
        features.message_count,
    ) {
        (true, count) if count <= 5 => Some(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 16,
            label: "atypical_feminine_first_name",
            reason: "New user profile has a feminine first-name pattern atypical for this chat baseline",
        }),
        _ => None,
    }
}
fn homoglyph_profile_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    let has_homoglyphs = [
        features.first_name.as_deref(),
        features.last_name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(has_mixed_script_homoglyphs);
    has_homoglyphs.then_some(RiskSignal {
        class: SpamClass::LlmProfileBait,
        coefficient: 3,
        label: "mixed_script_profile_homoglyphs",
        reason: "Profile name mixes Latin and Cyrillic look-alike letters",
    })
}
fn message_texture_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    match (
        features.text_texture.max_reuse_count,
        features.text_texture.duplicate_normalized_count,
        features.text_texture.max_pairwise_similarity,
        features.message_count,
    ) {
        (reuse_count, _, _, _) if reuse_count > 1 => Some(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 28,
            label: "duplicate_message_text",
            reason: "New user posted exactly repeated normalized messages",
        }),
        (_, duplicate_count, _, _) if duplicate_count > 0 => Some(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 22,
            label: "duplicate_message_texture",
            reason: "New user has duplicate normalized message texture",
        }),
        (_, _, Some(similarity), count) if similarity >= 0.86 && count >= 2 => Some(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 16,
            label: "similar_message_texture",
            reason: "Several new-user messages are unusually similar by text texture",
        }),
        _ => None,
    }
}
fn chat_position_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    match (
        features.message_count,
        features.top_level_message_count,
        features.reply_to_channel_post_count,
        features.reply_to_bot_count,
        features.reply_to_comment_count,
    ) {
        (count, _, channel_comments, 0, 0) if count > 0 && channel_comments == count => {
            Some(RiskSignal {
                class: SpamClass::LlmProfileBait,
                coefficient: 12,
                label: "only_channel_post_comments",
                reason: "New user only comments under channel posts",
            })
        }
        (_, _, _, bot_replies, _) if bot_replies > 0 => Some(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 5,
            label: "reply_to_bot_comment",
            reason: "New user replied to a bot first-comment thread",
        }),
        _ => None,
    }
}
fn message_style_signals(features: &NewUserFeatures) -> Vec<RiskSignal> {
    let style = &features.message_style;
    let persona = message_style_persona(features);
    let mut signals = Vec::new();

    if style.single_exclamation_ending_count > 0 {
        signals.push(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: message_style_coefficient(persona, 2, 5, 1),
            label: "single_exclamation_ending",
            reason: "New-user message ends with exactly one exclamation mark",
        });
    }
    if style.repeated_exclamation_ending_count > 0 {
        signals.push(human_style_signal(
            "repeated_exclamation_ending",
            "New-user message ends with several exclamation marks",
            -5,
        ));
    }
    if features.reply_to_channel_post_count > 0 && features.reply_to_comment_count == 0 {
        signals.push(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: message_style_coefficient(persona, 4, 10, 2),
            label: "reply_to_channel_post_not_comment",
            reason: "New user replies directly to a channel post, not another comment",
        });
    }
    if style.emoji_message_count > 0 && style.adjacent_emoji_message_count == 0 {
        signals.push(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: message_style_coefficient(persona, 1, 3, -1),
            label: "non_adjacent_emoji_message",
            reason: "New-user message uses emoji without an adjacent emoji run",
        });
    }
    if style.emoji_ending_count > 0 && style.adjacent_emoji_message_count == 0 {
        signals.push(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: message_style_coefficient(persona, 2, 5, 0),
            label: "non_adjacent_emoji_message_ending",
            reason: "New-user message ends with emoji but has no adjacent emoji run",
        });
    }
    if style.adjacent_emoji_message_count > 0 {
        signals.push(human_style_signal(
            "adjacent_emoji_run",
            "New-user message contains adjacent emoji",
            -5,
        ));
    }
    if style.period_ending_count > 0 || style.other_non_text_ending_count > 0 {
        signals.push(human_style_signal(
            "non_text_message_ending",
            "New-user message ends with non-text punctuation other than emoji",
            -3,
        ));
    }
    if style.unmatched_closing_parenthesis_ending_count > 0 {
        signals.push(human_style_signal(
            "russian_unmatched_closing_parenthesis",
            "New-user message ends with a closing parenthesis without a matching opening parenthesis",
            -6,
        ));
    }

    signals
}
fn human_style_signal(label: &'static str, reason: &'static str, coefficient: i32) -> RiskSignal {
    RiskSignal {
        class: SpamClass::LlmProfileBait,
        coefficient,
        label,
        reason,
    }
}
fn message_style_coefficient(
    persona: MessageStylePersona,
    general: i32,
    generic_feminine: i32,
    success_persona: i32,
) -> i32 {
    match persona {
        MessageStylePersona::General => general,
        MessageStylePersona::GenericFeminine => generic_feminine,
        MessageStylePersona::SuccessPersona => success_persona,
    }
}
pub fn message_style_persona(features: &NewUserFeatures) -> MessageStylePersona {
    if has_success_persona_markers(features) {
        MessageStylePersona::SuccessPersona
    } else if looks_like_feminine_first_name(features.first_name.as_deref()) {
        MessageStylePersona::GenericFeminine
    } else {
        MessageStylePersona::General
    }
}
fn has_success_persona_markers(features: &NewUserFeatures) -> bool {
    let profile_text = format!(
        "{}\n{}\n{}\n{}",
        features.display_name.as_deref().unwrap_or_default(),
        features.bio.as_deref().unwrap_or_default(),
        features
            .personal_channel_title
            .as_deref()
            .unwrap_or_default(),
        features
            .personal_channel_last_text
            .as_deref()
            .unwrap_or_default(),
    )
    .to_lowercase();
    [
        "бизнес",
        "доход",
        "заработ",
        "инвест",
        "крипт",
        "трейдер",
        "успеш",
        "предприним",
        "financial",
        "invest",
        "crypto",
        "trader",
    ]
    .iter()
    .any(|marker| profile_text.contains(marker))
}
fn reply_to_comment_mitigation_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    (features.reply_to_comment_count > 0).then_some(RiskSignal {
        class: SpamClass::LlmProfileBait,
        coefficient: -18,
        label: "reply_to_comment_reduces_generic_spam_risk",
        reason: "Replying to an existing comment is strong evidence of genuine chat participation",
    })
}
fn chat_age_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    match (features.chat_age_sec, features.message_count) {
        (Some(age), count) if age < 6 * 60 * 60 && count <= 5 => Some(RiskSignal {
            class: SpamClass::FreshAccount,
            coefficient: 8,
            label: "very_new_to_chat",
            reason: "User was first seen in this chat less than six hours ago",
        }),
        (Some(age), count) if age < 24 * 60 * 60 && count <= 5 => Some(RiskSignal {
            class: SpamClass::FreshAccount,
            coefficient: 4,
            label: "new_to_chat_today",
            reason: "User was first seen in this chat less than a day ago",
        }),
        _ => None,
    }
}
pub fn only_replies_or_comments(features: &NewUserFeatures) -> bool {
    matches!(
        (
            features.message_count,
            features.top_level_message_count,
            features.reply_to_channel_post_count,
            features.reply_to_bot_count,
            features.reply_to_comment_count,
        ),
        (count, 0, channel_comments, bot_replies, comment_replies)
            if count > 0 && channel_comments + bot_replies + comment_replies >= count
    )
}
pub fn only_channel_post_comments(features: &NewUserFeatures) -> bool {
    matches!(
        (
            features.message_count,
            features.reply_to_channel_post_count,
            features.reply_to_bot_count,
            features.reply_to_comment_count,
        ),
        (count, channel_comments, 0, 0) if count > 0 && channel_comments == count
    )
}
fn personal_channel_signals(features: &NewUserFeatures) -> Vec<RiskSignal> {
    let mut signals = Vec::new();

    if features.personal_channel_has_adult_links {
        signals.push(RiskSignal {
            class: SpamClass::AdultPersonalChannel,
            coefficient: 55,
            label: "personal_channel_adult_links",
            reason: "Attached personal channel contains adult/invite promo links",
        });
    }

    if personal_channel_has_invite_link(features) {
        signals.push(RiskSignal {
            class: SpamClass::LinkDropper,
            coefficient: 20,
            label: "personal_channel_invite_link",
            reason: "Attached personal channel contains Telegram invite links",
        });
    }

    if personal_channel_has_external_link(features) {
        signals.push(RiskSignal {
            class: SpamClass::LinkDropper,
            coefficient: 8,
            label: "personal_channel_external_link",
            reason: "Attached personal channel contains an external link",
        });
    }

    if let Some(signal) = personal_channel_title_reuse_signal(
        features.personal_channel_title_reuse_spammer_count,
        features.personal_channel_title_reuse_count,
        features.message_count,
    ) {
        signals.push(signal);
    }

    signals
}
fn personal_channel_title_reuse_signal(
    spammer_count: i64,
    reuse_count: i64,
    message_count: i64,
) -> Option<RiskSignal> {
    match (spammer_count, reuse_count, message_count) {
        (spammer_count, _, _) if spammer_count > 0 => Some(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 24,
            label: "personal_channel_title_reused_by_spammers",
            reason: "Personal channel title has already appeared on manually marked spammers",
        }),
        (0, reuse_count, message_count) if reuse_count > 0 && message_count <= 3 => {
            Some(RiskSignal {
                class: SpamClass::LlmProfileBait,
                coefficient: 10,
                label: "personal_channel_title_reused_by_new_accounts",
                reason: "Personal channel title is reused by other seen accounts",
            })
        }
        _ => None,
    }
}
fn short_bio_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    match (
        features.bio.as_deref().map(str::chars).map(Iterator::count),
        features.message_count,
    ) {
        (Some(0..=4), count) if count <= 3 => Some(RiskSignal {
            class: SpamClass::FreshAccount,
            coefficient: 4,
            label: "very_short_bio",
            reason: "Very short bio on a new account",
        }),
        _ => None,
    }
}
fn explicit_adult_bio_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    let bio = features.bio.as_deref()?;
    has_explicit_adult_promo_bio(bio).then_some(RiskSignal {
        class: SpamClass::AdultPersonalChannel,
        coefficient: 42,
        label: "explicit_adult_promo_bio",
        reason: "Profile bio promotes an explicit adult service together with a link or funnel cue",
    })
}
fn profile_bio_subscription_offer_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    let bio = features.bio.as_deref()?;
    has_subscription_offer_bio(bio).then_some(RiskSignal {
        class: SpamClass::PromoDmBait,
        coefficient: 45,
        label: "profile_bio_subscription_invite_offer",
        reason: "Profile bio advertises a paid digital subscription through a Telegram invite link",
    })
}
fn has_explicit_adult_promo_bio(bio: &str) -> bool {
    let raw = bio.to_lowercase();
    let normalized = normalize_cyrillic_homoglyphs(bio).to_lowercase();
    let adult_provider = ["onlyfans", "онлифанс", "fansly", "pornhub", "xvideos"]
        .iter()
        .any(|marker| raw.contains(marker) || normalized.contains(marker));
    let promotional_context = [
        "t.me/",
        "telegram.me/",
        "onlyfans.com",
        "fansly.com",
        "слив",
        "hot",
    ]
    .iter()
    .any(|marker| raw.contains(marker) || normalized.contains(marker));
    adult_provider && promotional_context
}
fn has_subscription_offer_bio(bio: &str) -> bool {
    let raw = bio.to_lowercase();
    let normalized = normalize_cyrillic_homoglyphs(bio).to_lowercase();
    let variants = [&raw, &normalized];
    let has_invite_link = variants
        .iter()
        .any(|value| value.contains("t.me/+") || value.contains("telegram.me/+"));
    let has_subscription_product = [
        "gemini",
        "gеmіni",
        "chatgpt",
        "openai",
        "claude",
        "midjourney",
        "spotify",
        "youtube premium",
    ]
    .iter()
    .any(|marker| variants.iter().any(|value| value.contains(marker)));
    let has_sales_term = ["pro", "подпис", "месяц", "год", "$", "руб", "₽", "за "]
        .iter()
        .any(|marker| variants.iter().any(|value| value.contains(marker)));
    has_invite_link && has_subscription_product && has_sales_term
}
fn member_status_signal(features: &NewUserFeatures) -> Option<RiskSignal> {
    match features.member_status.as_deref() {
        Some("left" | "banned") => Some(RiskSignal {
            class: SpamClass::FreshAccount,
            coefficient: 6,
            label: "not_present_in_chat",
            reason: "Latest member snapshot says user is no longer present",
        }),
        _ => None,
    }
}
#[derive(Debug, Clone)]
pub struct UsernameStats {
    pub has_digits: bool,
    pub digit_count: i32,
    pub has_random_suffix: bool,
    pub pattern: String,
}
pub fn username_stats(username: Option<&str>) -> UsernameStats {
    let Some(username) = username.map(str::trim).filter(|value| !value.is_empty()) else {
        return UsernameStats {
            has_digits: false,
            digit_count: 0,
            has_random_suffix: false,
            pattern: "missing".to_string(),
        };
    };

    let digit_count = username.chars().filter(|ch| ch.is_ascii_digit()).count() as i32;
    let has_digits = digit_count > 0;
    let lower = username.to_lowercase();
    let parts = lower.split('_').collect::<Vec<_>>();
    let suffix = parts.last().copied().unwrap_or_default();
    let has_random_suffix = parts.len() >= 2
        && (3..=8).contains(&suffix.len())
        && suffix
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        && (suffix.chars().any(|ch| ch.is_ascii_digit())
            || suffix.chars().filter(|ch| "aeiouy".contains(*ch)).count() <= 1);
    let pattern = if has_random_suffix {
        "random_suffix"
    } else if has_digits {
        "contains_digits"
    } else {
        "plain"
    };

    UsernameStats {
        has_digits,
        digit_count,
        has_random_suffix,
        pattern: pattern.to_string(),
    }
}
pub fn looks_like_feminine_first_name(first_name: Option<&str>) -> bool {
    let Some(first_name) = first_name.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let normalized = canonicalize_feminine_name(first_name)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(|ch: char| !ch.is_alphabetic())
        .to_lowercase();

    matches!(
        normalized.as_str(),
        "abby"
            | "abigail"
            | "ada"
            | "adeline"
            | "alice"
            | "alina"
            | "alisa"
            | "alyssa"
            | "amanda"
            | "amelia"
            | "amy"
            | "anna"
            | "anne"
            | "annie"
            | "ariana"
            | "audrey"
            | "ava"
            | "bella"
            | "camila"
            | "caroline"
            | "charlotte"
            | "chloe"
            | "claire"
            | "daisy"
            | "diana"
            | "ella"
            | "ellie"
            | "emily"
            | "emma"
            | "eva"
            | "evelyn"
            | "grace"
            | "hannah"
            | "helen"
            | "irene"
            | "isabella"
            | "jane"
            | "jessica"
            | "julia"
            | "kate"
            | "katherine"
            | "katie"
            | "lana"
            | "laura"
            | "lily"
            | "linda"
            | "lucy"
            | "maria"
            | "marie"
            | "mary"
            | "mia"
            | "mila"
            | "natalie"
            | "nicole"
            | "olivia"
            | "rachel"
            | "rebecca"
            | "sarah"
            | "scarlett"
            | "sophia"
            | "stella"
            | "susan"
            | "victoria"
            | "violet"
            | "zoe"
            | "аврора"
            | "агата"
            | "александра"
            | "алена"
            | "алина"
            | "алиса"
            | "алла"
            | "альбина"
            | "анастасия"
            | "ангелина"
            | "анна"
            | "антонина"
            | "арина"
            | "валентина"
            | "валерия"
            | "вера"
            | "вероника"
            | "виктория"
            | "галина"
            | "дарья"
            | "диана"
            | "екатерина"
            | "елена"
            | "елизавета"
            | "жанна"
            | "зоя"
            | "инна"
            | "ирина"
            | "карина"
            | "катя"
            | "кира"
            | "кора"
            | "ксения"
            | "лана"
            | "лариса"
            | "лена"
            | "лилия"
            | "любовь"
            | "людмила"
            | "маргарита"
            | "марина"
            | "мария"
            | "милана"
            | "надежда"
            | "наталья"
            | "ника"
            | "нино"
            | "нина"
            | "оксана"
            | "ольга"
            | "оля"
            | "olga"
            | "olya"
            | "полина"
            | "светлана"
            | "софия"
            | "таисия"
            | "таня"
            | "танюша"
            | "татьяна"
            | "ульяна"
            | "юлия"
            | "яна"
    )
}
fn canonicalize_feminine_name(value: &str) -> String {
    let normalized = normalize_cyrillic_homoglyphs(value).to_lowercase();
    // После замены гомоглифов `Оlya` превращается в `оlуа`.
    normalized.replace("lуа", "ля").replace("lя", "ля")
}
pub fn has_profile_photo(features: &NewUserFeatures) -> bool {
    features.profile_photo_file_unique_id.is_some()
        || features.profile_photo_file_id.is_some()
        || features.profile_photo_count.unwrap_or_default() > 0
}
pub fn personal_channel_has_invite_link(features: &NewUserFeatures) -> bool {
    let text = personal_channel_text_blob(features).to_lowercase();
    text.contains("t.me/+") || text.contains("telegram.me/+")
}
pub fn personal_channel_has_external_link(features: &NewUserFeatures) -> bool {
    let text = personal_channel_text_blob(features).to_lowercase();
    text.contains("http://") || text.contains("https://") || text.contains("t.me/")
}
fn user_message_text_blob(features: &NewUserFeatures) -> String {
    let mut parts = Vec::new();
    parts.extend(features.first_message_text.iter().map(String::as_str));
    parts.extend(features.last_message_text.iter().map(String::as_str));
    parts.extend(features.recent_message_texts.iter().map(String::as_str));
    parts.join("\n")
}
pub fn message_style(texts: &[String]) -> MessageStyle {
    texts
        .iter()
        .fold(MessageStyle::default(), |mut style, text| {
            let text = text.trim_end();
            if text.is_empty() {
                return style;
            }

            style.text_message_count += 1;
            let trailing_exclamation_count = trailing_char_count(text, '!');
            if trailing_exclamation_count == 1 {
                style.single_exclamation_ending_count += 1;
            }
            if trailing_exclamation_count >= 2 {
                style.repeated_exclamation_ending_count += 1;
            }
            if trailing_char_count(text, '.') == 1 {
                style.period_ending_count += 1;
            }
            if text.chars().any(is_emoji) {
                style.emoji_message_count += 1;
            }
            if ends_with_emoji(text) {
                style.emoji_ending_count += 1;
            }
            if emoji_scalar_count(text) == 1 {
                style.single_emoji_message_count += 1;
                if ends_with_emoji(text) {
                    style.single_emoji_ending_count += 1;
                }
            }
            if has_adjacent_emoji_clusters(text) {
                style.adjacent_emoji_message_count += 1;
            }
            if ends_with_other_non_text(text) {
                style.other_non_text_ending_count += 1;
            }
            if ends_with_unmatched_closing_parenthesis(text) {
                style.unmatched_closing_parenthesis_ending_count += 1;
            }
            style
        })
}
fn trailing_char_count(text: &str, expected: char) -> usize {
    text.chars().rev().take_while(|ch| *ch == expected).count()
}
fn ends_with_emoji(text: &str) -> bool {
    let mut chars = text.trim_end().chars().rev();
    let Some(last) = chars.next() else {
        return false;
    };
    if is_emoji(last) {
        return true;
    }
    matches!(last as u32, 0xFE0F | 0x1F3FB..=0x1F3FF) && chars.next().is_some_and(is_emoji)
}
fn is_emoji(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2300..=0x23FF | 0x2B00..=0x2BFF
    )
}
fn emoji_scalar_count(text: &str) -> usize {
    text.chars().filter(|ch| is_emoji(*ch)).count()
}
fn has_adjacent_emoji_clusters(text: &str) -> bool {
    let mut previous_was_emoji = false;
    let mut joined_with_zwj = false;

    for ch in text.chars() {
        if is_emoji(ch) && !is_emoji_modifier(ch) {
            if previous_was_emoji && !joined_with_zwj {
                return true;
            }
            previous_was_emoji = true;
            joined_with_zwj = false;
        } else if is_emoji_modifier(ch) || ch == '\u{FE0F}' {
            continue;
        } else if ch == '\u{200D}' && previous_was_emoji {
            joined_with_zwj = true;
        } else {
            previous_was_emoji = false;
            joined_with_zwj = false;
        }
    }

    false
}
fn is_emoji_modifier(ch: char) -> bool {
    matches!(ch as u32, 0x1F3FB..=0x1F3FF)
}
fn ends_with_other_non_text(text: &str) -> bool {
    let Some(last) = text.trim_end().chars().last() else {
        return false;
    };
    !last.is_alphanumeric() && !is_emoji(last) && !matches!(last, '!' | ')' | '.')
}
fn ends_with_unmatched_closing_parenthesis(text: &str) -> bool {
    let text = text.trim_end();
    text.ends_with(')')
        && text.chars().filter(|ch| *ch == ')').count()
            > text.chars().filter(|ch| *ch == '(').count()
}
fn contains_cjk(text: &str) -> bool {
    text.chars()
        .any(|ch| matches!(ch as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x3040..=0x30FF))
}
fn personal_channel_text_blob(features: &NewUserFeatures) -> String {
    format!(
        "{}\n{}\n{}",
        features
            .personal_channel_title
            .as_deref()
            .unwrap_or_default(),
        features
            .personal_channel_username
            .as_deref()
            .unwrap_or_default(),
        features
            .personal_channel_last_text
            .as_deref()
            .unwrap_or_default()
    )
}
fn normalize_message_text(text: &str) -> Option<String> {
    let normalized = text
        .chars()
        .map(|ch| match ch {
            ch if ch.is_alphanumeric() => ch.to_lowercase().collect::<String>(),
            ch if ch.is_whitespace() => " ".to_string(),
            _ => " ".to_string(),
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    match normalized.is_empty() {
        true => None,
        false => Some(normalized),
    }
}
pub fn max_pairwise_message_similarity(texts: &[String]) -> Option<f64> {
    let normalized = texts
        .iter()
        .filter_map(|text| normalize_message_text(text))
        .filter(|text| text.chars().count() >= 8)
        .take(20)
        .collect::<Vec<_>>();

    match normalized.len() {
        0 | 1 => None,
        len => (0..len)
            .flat_map(|left| ((left + 1)..len).map(move |right| (left, right)))
            .map(|(left, right)| message_similarity(&normalized[left], &normalized[right]))
            .max_by(|left, right| left.total_cmp(right)),
    }
}
fn message_similarity(left: &str, right: &str) -> f64 {
    match left == right {
        true => 1.0,
        false => token_jaccard(left, right).max(char_ngram_jaccard(left, right, 3)),
    }
}
fn token_jaccard(left: &str, right: &str) -> f64 {
    let left_tokens = left
        .split_whitespace()
        .collect::<std::collections::BTreeSet<_>>();
    let right_tokens = right
        .split_whitespace()
        .collect::<std::collections::BTreeSet<_>>();
    jaccard(&left_tokens, &right_tokens)
}
fn char_ngram_jaccard(left: &str, right: &str, width: usize) -> f64 {
    let left_ngrams = char_ngrams(left, width);
    let right_ngrams = char_ngrams(right, width);
    jaccard(&left_ngrams, &right_ngrams)
}
fn char_ngrams(text: &str, width: usize) -> std::collections::BTreeSet<String> {
    let chars = text.chars().collect::<Vec<_>>();
    match chars.len() < width {
        true => std::iter::once(text.to_string()).collect(),
        false => chars
            .windows(width)
            .map(|window| window.iter().collect::<String>())
            .collect(),
    }
}
fn jaccard<T: Ord>(
    left: &std::collections::BTreeSet<T>,
    right: &std::collections::BTreeSet<T>,
) -> f64 {
    match (left.is_empty(), right.is_empty()) {
        (true, true) => 1.0,
        (true, false) | (false, true) => 0.0,
        (false, false) => {
            let intersection = left.intersection(right).count() as f64;
            let union = left.union(right).count() as f64;
            intersection / union
        }
    }
}
pub fn char_count_i32(value: &str) -> i32 {
    i32::try_from(value.chars().count()).unwrap_or(i32::MAX)
}
pub fn id_bucket(user_id: i64) -> String {
    match user_id {
        0..=999_999_999 => "lt_1b",
        1_000_000_000..=1_999_999_999 => "1b_2b",
        2_000_000_000..=4_999_999_999 => "2b_5b",
        5_000_000_000..=7_999_999_999 => "5b_8b",
        8_000_000_000..=9_999_999_999 => "8b_10b",
        _ => "gte_10b",
    }
    .to_string()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_channel_post_comments_are_a_high_lift_signal() {
        let features = NewUserFeatures {
            message_count: 2,
            reply_to_channel_post_count: 2,
            ..Default::default()
        };

        let signal = chat_position_signal(&features).expect("direct post comments are scored");
        assert_eq!(signal.label, "only_channel_post_comments");
        assert_eq!(signal.coefficient, 12);
    }

    #[test]
    fn ordinary_comment_thread_participation_is_not_a_spam_signal() {
        let features = NewUserFeatures {
            message_count: 2,
            reply_to_comment_count: 2,
            ..Default::default()
        };

        assert!(chat_position_signal(&features).is_none());
    }

    #[test]
    fn channel_content_is_scored_but_attachment_itself_is_not() {
        let features = NewUserFeatures {
            message_count: 1,
            reply_to_channel_post_count: 1,
            chat_age_sec: Some(60),
            personal_channel_chat_id: Some(-100_000_000_001),
            personal_channel_last_text: Some("https://example.org".to_string()),
            ..Default::default()
        };

        let signals = personal_channel_signals(&features);
        assert!(
            signals
                .iter()
                .all(|signal| signal.label != "personal_channel_attached")
        );
        assert_eq!(
            signals
                .iter()
                .find(|signal| signal.label == "personal_channel_external_link")
                .map(|signal| signal.coefficient),
            Some(8)
        );
        assert_eq!(
            shared_spam_decision_tree(&features, &NewUserAnalysisConfig::default())
                .map(|leaf| leaf.label),
            Some("tree_fresh_channel_external_link")
        );
    }

    #[test]
    fn decision_tree_requires_combinations_for_generic_comment_and_id_signals() {
        let comments_with_channel = NewUserFeatures {
            message_count: 2,
            reply_to_channel_post_count: 2,
            personal_channel_chat_id: Some(-100_000_000_001),
            ..Default::default()
        };
        assert!(
            shared_spam_decision_tree(&comments_with_channel, &NewUserAnalysisConfig::default())
                .is_none()
        );

        let comments_with_recent_id = NewUserFeatures {
            message_count: 2,
            reply_to_channel_post_count: 2,
            id_rank_ratio: Some(0.99),
            ..Default::default()
        };
        assert_eq!(
            shared_spam_decision_tree(&comments_with_recent_id, &NewUserAnalysisConfig::default())
                .map(|leaf| leaf.label),
            Some("tree_channel_comments_with_recent_id")
        );

        let recent_id_with_random_username = NewUserFeatures {
            id_rank_ratio: Some(0.99),
            username: Some("dev_yasnyy_dcpc".to_string()),
            ..Default::default()
        };
        assert_eq!(
            shared_spam_decision_tree(
                &recent_id_with_random_username,
                &NewUserAnalysisConfig::default()
            )
            .map(|leaf| leaf.label),
            Some("tree_recent_id_random_username")
        );

        let only_a_comment = NewUserFeatures {
            message_count: 1,
            reply_to_comment_count: 1,
            ..Default::default()
        };
        assert!(
            shared_spam_decision_tree(&only_a_comment, &NewUserAnalysisConfig::default()).is_none()
        );
    }

    #[test]
    fn invite_or_adult_funnel_is_a_direct_tree_leaf() {
        let invite_channel = NewUserFeatures {
            personal_channel_chat_id: Some(-100_000_000_001),
            personal_channel_last_text: Some("Подписывайтесь t.me/+invite".to_string()),
            ..Default::default()
        };
        let leaf = shared_spam_decision_tree(&invite_channel, &NewUserAnalysisConfig::default())
            .expect("attached-channel invite funnel is a decisive path");
        assert_eq!(leaf.label, "tree_personal_channel_invite_funnel");

        let adult_channel = NewUserFeatures {
            personal_channel_chat_id: Some(-100_000_000_001),
            personal_channel_has_adult_links: true,
            ..Default::default()
        };
        let leaf = shared_spam_decision_tree(&adult_channel, &NewUserAnalysisConfig::default())
            .expect("adult channel funnel is a decisive path");
        assert_eq!(leaf.label, "tree_personal_channel_adult_funnel");
    }

    #[test]
    fn personal_channel_presence_alone_does_not_raise_risk() {
        let profile_without_channel = NewUserFeatures {
            message_count: 1,
            username: Some("roman_cedar_w6aepzfs".to_string()),
            ..Default::default()
        };
        let mut profile_with_channel = profile_without_channel.clone();
        profile_with_channel.personal_channel_chat_id = Some(-100_000_000_001);

        let without_channel = analyze_new_or_low_activity_user(
            &profile_without_channel,
            &NewUserAnalysisConfig::default(),
        );
        let with_channel = analyze_new_or_low_activity_user(
            &profile_with_channel,
            &NewUserAnalysisConfig::default(),
        );
        assert_eq!(with_channel.score, without_channel.score);
        assert_eq!(with_channel.signals, without_channel.signals);
        assert!(personal_channel_signals(&profile_with_channel).is_empty());
    }

    #[test]
    fn fresh_money_work_offer_needs_a_contact_cta_and_fresh_low_activity_context() {
        let crypto_recruitment = NewUserFeatures {
            message_count: 1,
            chat_age_sec: Some(1),
            first_message_text: Some(
                "Заработок на крипте на удалёнке. Простые задачи, обучаем с нуля, опыт не нужен. Пиши в лс @contact_name".to_string(),
            ),
            ..Default::default()
        };
        let leaf =
            shared_spam_decision_tree(&crypto_recruitment, &NewUserAnalysisConfig::default())
                .expect("fresh crypto recruitment with a direct CTA is a tree leaf");
        assert_eq!(leaf.label, "tree_fresh_money_work_contact_funnel");
        assert_eq!(
            leaf.path,
            &[
                "chat_age_under_twenty_four_hours",
                "up_to_three_messages",
                "money_or_work_offer",
                "direct_contact_call_to_action",
            ]
        );
        let analysis = analyze_new_or_low_activity_user(
            &crypto_recruitment,
            &NewUserAnalysisConfig::default(),
        );
        assert_eq!(analysis.score, 70);
        assert_eq!(analysis.level, "high");
        assert!(analysis.labels.contains(&leaf.label.to_string()));

        let paid_task = NewUserFeatures {
            message_count: 1,
            chat_age_sec: Some(1),
            first_message_text: Some(
                "Дам 5000 за пару легских движений, жду @contact_name".to_string(),
            ),
            ..Default::default()
        };
        assert_eq!(
            shared_spam_decision_tree(&paid_task, &NewUserAnalysisConfig::default())
                .map(|leaf| leaf.label),
            Some("tree_fresh_paid_task_offer")
        );

        let paid_task_without_contact_cta = NewUserFeatures {
            message_count: 1,
            chat_age_sec: Some(1),
            first_message_text: Some("Дам 1800 за небольшую помощь".to_string()),
            ..Default::default()
        };
        assert_eq!(
            shared_spam_decision_tree(
                &paid_task_without_contact_cta,
                &NewUserAnalysisConfig::default()
            )
            .map(|leaf| leaf.label),
            Some("tree_fresh_paid_task_offer")
        );

        let stale_offer = NewUserFeatures {
            chat_age_sec: Some(25 * 60 * 60),
            first_message_text: crypto_recruitment.first_message_text.clone(),
            ..Default::default()
        };
        assert!(
            shared_spam_decision_tree(&stale_offer, &NewUserAnalysisConfig::default()).is_none()
        );

        let delayed_obvious_offer = NewUserFeatures {
            message_count: 2,
            chat_age_sec: Some(8 * 60 * 60),
            first_message_text: Some(
                "Есть вариант заработать! От 16 000р в день. Подойдёт любому — пиши @contact"
                    .to_string(),
            ),
            ..Default::default()
        };
        let delayed_analysis = analyze_new_or_low_activity_user(
            &delayed_obvious_offer,
            &NewUserAnalysisConfig::default(),
        );
        assert_eq!(delayed_analysis.score, 70);
        assert!(
            delayed_analysis
                .labels
                .contains(&"tree_fresh_money_work_contact_funnel".to_string())
        );

        let delayed_phone_work_offer = NewUserFeatures {
            message_count: 2,
            chat_age_sec: Some(8 * 60 * 60),
            first_message_text: Some(
                "Есть вариант заработать пару тысяч зеленых за месяц, нужно лишь телефон и твое время, пиши @contact".to_string(),
            ),
            ..Default::default()
        };
        assert_eq!(
            shared_spam_decision_tree(&delayed_phone_work_offer, &NewUserAnalysisConfig::default())
                .map(|leaf| leaf.label),
            Some("tree_fresh_money_work_contact_funnel")
        );

        let delayed_paid_task_offer = NewUserFeatures {
            message_count: 2,
            chat_age_sec: Some(8 * 60 * 60),
            first_message_text: Some(
                "50к за пару простых действий, черкани ему @contact".to_string(),
            ),
            ..Default::default()
        };
        assert_eq!(
            shared_spam_decision_tree(&delayed_paid_task_offer, &NewUserAnalysisConfig::default())
                .map(|leaf| leaf.label),
            Some("tree_fresh_paid_task_offer")
        );

        let ordinary_course_share = NewUserFeatures {
            chat_age_sec: Some(1),
            first_message_text: Some(
                "Прошёл курс по крипте, помогло на старте; пишите, скину бесплатно".to_string(),
            ),
            ..Default::default()
        };
        assert!(
            shared_spam_decision_tree(&ordinary_course_share, &NewUserAnalysisConfig::default())
                .is_none()
        );
    }

    #[test]
    fn fresh_direct_contact_send_offer_is_a_decisive_review_tree() {
        let audio_book_offer = NewUserFeatures {
            message_count: 1,
            chat_age_sec: Some(1),
            first_message_text: Some(
                "Есть хорошая аудиоверсия, если интересно — пишите в личку, отправлю.".to_string(),
            ),
            ..Default::default()
        };
        let leaf = shared_spam_decision_tree(&audio_book_offer, &NewUserAnalysisConfig::default())
            .expect("fresh direct-message content funnel is a decisive tree leaf");
        assert_eq!(leaf.label, "tree_fresh_contact_send_offer");
        assert_eq!(
            leaf.path,
            &[
                "chat_age_under_six_hours",
                "one_or_two_messages",
                "direct_contact_call_to_action",
                "promise_to_send_content",
            ]
        );
        let analysis =
            analyze_new_or_low_activity_user(&audio_book_offer, &NewUserAnalysisConfig::default());
        assert_eq!(analysis.score, 70);
        assert_eq!(analysis.level, "high");

        let no_contact_cta = NewUserFeatures {
            message_count: 1,
            chat_age_sec: Some(1),
            first_message_text: Some(
                "Есть хорошая аудиоверсия, отправлю ссылку позже.".to_string(),
            ),
            ..Default::default()
        };
        assert!(
            shared_spam_decision_tree(&no_contact_cta, &NewUserAnalysisConfig::default()).is_none()
        );

        let stale_offer = NewUserFeatures {
            chat_age_sec: Some(7 * 60 * 60),
            ..audio_book_offer
        };
        assert!(
            shared_spam_decision_tree(&stale_offer, &NewUserAnalysisConfig::default()).is_none()
        );
    }

    #[test]
    fn fresh_recent_id_repeated_message_is_a_decisive_review_tree() {
        let repeated_campaign = NewUserFeatures {
            message_count: 3,
            chat_age_sec: Some(31_932),
            id_rank_ratio: Some(0.946),
            text_texture: TextTexture {
                duplicate_normalized_count: 1,
                max_reuse_count: 2,
                repetitive_pattern: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let leaf = shared_spam_decision_tree(&repeated_campaign, &NewUserAnalysisConfig::default())
            .expect("fresh repeated campaign from a recent ID is a decisive tree leaf");
        assert_eq!(leaf.label, "tree_fresh_recent_id_repeated_message");
        assert_eq!(
            leaf.path,
            &[
                "chat_age_under_twenty_four_hours",
                "up_to_three_messages",
                "recent_telegram_id",
                "repeated_normalized_message",
            ]
        );

        let stale_campaign = NewUserFeatures {
            chat_age_sec: Some(25 * 60 * 60),
            ..repeated_campaign
        };
        assert!(
            shared_spam_decision_tree(&stale_campaign, &NewUserAnalysisConfig::default()).is_none()
        );
    }

    #[test]
    fn personal_channel_presence_does_not_trigger_a_decision_tree() {
        let features = NewUserFeatures {
            message_count: 2,
            reply_to_channel_post_count: 2,
            personal_channel_chat_id: Some(-100_000_000_001),
            ..Default::default()
        };

        let analysis =
            analyze_new_or_low_activity_user(&features, &NewUserAnalysisConfig::default());

        assert!(analysis.score < NewUserAnalysisConfig::default().review_threshold);
        assert_eq!(analysis.level, "low");
        assert!(
            analysis
                .signals
                .as_array()
                .is_some_and(|signals| signals.iter().all(|signal| {
                    signal["decision_tree_version"] != SHARED_SPAM_DECISION_TREE_VERSION
                }))
        );
    }

    #[test]
    fn shared_spammer_identity_is_a_review_threshold_signal() {
        let signal = shared_spammer_signal(true).expect("shared spammer must have a signal");
        assert_eq!(signal.class, SpamClass::KnownSpammer);
        assert_eq!(signal.coefficient, 70);
        assert_eq!(signal.label, "shared_spammer_identity");
        assert!(shared_spammer_signal(false).is_none());
    }

    #[test]
    fn lols_spammer_identity_scores_below_own_reputation() {
        let signal = lols_spammer_signal(true).expect("lols spammer must have a signal");
        assert_eq!(signal.class, SpamClass::KnownSpammer);
        assert_eq!(signal.coefficient, 50);
        assert_eq!(signal.label, "lols_spammer_identity");
        assert!(lols_spammer_signal(false).is_none());
    }

    #[test]
    fn telegram_id_risk_signal_scales_configured_four_pl_probability() {
        let model = TelegramIdRiskModel {
            floor: 0.1,
            ceil: 0.9,
            k: 4.0,
            midpoint_billion: 8.0,
            version: "test-4pl".to_string(),
        };
        let lower = telegram_id_spam_probability(7_000_000_000, &model);
        let middle = telegram_id_spam_probability(8_000_000_000, &model);
        let upper_middle = telegram_id_spam_probability(8_500_000_000, &model);
        let upper = telegram_id_spam_probability(9_000_000_000, &model);

        assert!(lower < middle && middle < upper_middle && upper_middle < upper);
        assert!((middle - 0.5).abs() < 1e-9);
        assert_eq!(telegram_id_risk_coefficient(lower), 0);
        assert_eq!(telegram_id_risk_coefficient(middle), 8);
        assert_eq!(telegram_id_risk_coefficient(upper_middle), 14);
        assert_eq!(telegram_id_risk_coefficient(upper), 15);
        assert_eq!(telegram_id_risk_coefficient(0.09), 0);
        assert_eq!(telegram_id_risk_coefficient(0.86), 15);
    }

    #[test]
    fn username_random_suffix_detects_yasnyy_variant() {
        let stats = username_stats(Some("dev_yasnyy_dcpc"));
        assert!(stats.has_random_suffix);
        assert_eq!(stats.pattern, "random_suffix");
    }

    #[test]
    fn username_plain_does_not_look_random() {
        let stats = username_stats(Some("Chechulinm"));
        assert!(!stats.has_random_suffix);
        assert_eq!(stats.pattern, "plain");
    }

    #[test]
    fn feminine_name_pattern_detects_known_feminine_names_conservatively() {
        assert!(looks_like_feminine_first_name(Some("Мария")));
        assert!(looks_like_feminine_first_name(Some("Анна")));
        assert!(looks_like_feminine_first_name(Some("Нино ❤️")));
        assert!(looks_like_feminine_first_name(Some("Лана 💻")));
        assert!(looks_like_feminine_first_name(Some("Кора 🌊")));
        assert!(looks_like_feminine_first_name(Some("Tанюша")));
        assert!(looks_like_feminine_first_name(Some("Оlya")));
        assert!(looks_like_feminine_first_name(Some("Olya")));
        assert!(looks_like_feminine_first_name(Some("Alice")));
        assert!(looks_like_feminine_first_name(Some("Mary Johnson")));
        assert!(looks_like_feminine_first_name(Some("Sophia")));
        assert!(!looks_like_feminine_first_name(Some("Nick")));
        assert!(!looks_like_feminine_first_name(Some("Daniel")));
        assert!(!looks_like_feminine_first_name(Some("Alex")));
        assert!(!looks_like_feminine_first_name(Some("Никита")));
        assert!(!looks_like_feminine_first_name(Some("Данила")));
        assert!(!looks_like_feminine_first_name(Some("Илья")));
        assert!(!looks_like_feminine_first_name(Some("Дима")));
        assert!(!looks_like_feminine_first_name(Some("Чат")));
    }

    #[test]
    fn adult_bio_requires_provider_and_funnel() {
        assert!(has_explicit_adult_promo_bio(
            "Канал с горячими сливами OnlyFans моделей 🔥 t.me/example"
        ));
        assert!(!has_explicit_adult_promo_bio(
            "OnlyFans is a subscription platform"
        ));
        assert!(!has_explicit_adult_promo_bio(
            "Горячие новости t.me/example"
        ));
    }

    #[test]
    fn text_similarity_detects_repeated_template() {
        let texts = vec![
            "одинаковая структура сообщения с небольшим изменением".to_string(),
            "одинаковая структура сообщения — с небольшим изменением".to_string(),
        ];
        assert!(max_pairwise_message_similarity(&texts).is_some_and(|score| score >= 0.86));
    }

    #[test]
    fn text_similarity_ignores_normal_different_messages() {
        let texts = vec![
            "а видеокарты для майнеров уже подорожали на 100% в 2021".to_string(),
            "и кто теперь будет подбирать пин на восьмой попытке.".to_string(),
        ];
        assert!(max_pairwise_message_similarity(&texts).is_some_and(|score| score < 0.86));
    }

    #[test]
    fn message_style_keeps_weak_punctuation_and_emoji_signals_separate() {
        let style = message_style(&[
            "Спасибо!".to_string(),
            "Очень интересно!!".to_string(),
            "Классно 🌊".to_string(),
            "Обычное предложение.".to_string(),
        ]);

        assert_eq!(style.text_message_count, 4);
        assert_eq!(style.single_exclamation_ending_count, 1);
        assert_eq!(style.period_ending_count, 1);
        assert_eq!(style.emoji_message_count, 1);
        assert_eq!(style.emoji_ending_count, 1);
        assert_eq!(style.single_emoji_message_count, 1);
        assert_eq!(style.single_emoji_ending_count, 1);
    }

    #[test]
    fn message_style_uses_single_emoji_not_any_emoji_count() {
        assert_eq!(emoji_scalar_count("один ❤️"), 1);
        assert_eq!(emoji_scalar_count("два 🌊❤️"), 2);
    }

    #[test]
    fn human_style_patterns_detect_adjacent_emoji_and_unmatched_parentheses() {
        assert!(!has_adjacent_emoji_clusters("сначала 🌊 потом ❤️"));
        assert!(has_adjacent_emoji_clusters("сразу 🌊❤️"));
        assert!(!has_adjacent_emoji_clusters("разработчица 👩‍💻"));
        assert!(ends_with_unmatched_closing_parenthesis("ну да)"));
        assert!(!ends_with_unmatched_closing_parenthesis("(ну да)"));
        assert!(ends_with_other_non_text("что-то?"));
        assert!(!ends_with_other_non_text("обычная точка."));
    }

    #[test]
    fn message_style_weights_apply_to_every_persona() {
        assert_eq!(
            message_style_coefficient(MessageStylePersona::General, 2, 5, 1),
            2
        );
        assert_eq!(
            message_style_coefficient(MessageStylePersona::GenericFeminine, 2, 5, 1),
            5
        );
        assert_eq!(
            message_style_coefficient(MessageStylePersona::SuccessPersona, 1, 3, -1),
            -1
        );
    }

    #[test]
    fn risk_signals_record_warning_strength_and_coefficient() {
        let mut risk = RiskAccumulator::default();
        risk.add(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 3,
            label: "weak_style_signal",
            reason: "test",
        });
        risk.add(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: -18,
            label: "genuine_reply",
            reason: "test",
        });
        let signals = risk.finish(70).signals;

        assert_eq!(signals[0]["warning_strength"], "weak");
        assert_eq!(signals[0]["coefficient"], 3);
        assert_eq!(signals[1]["warning_strength"], "mitigating");
        assert_eq!(signals[1]["coefficient"], -18);
    }

    #[test]
    fn cjk_detector_catches_foreign_invite_seed() {
        assert!(contains_cjk("只要節奏對了大肉吃飽"));
        assert!(!contains_cjk("обычный русский текст"));
    }

    #[test]
    fn subscription_offer_bio_detects_homoglyph_gemini_invite() {
        assert!(has_subscription_offer_bio(
            "Gеmіni Pro на 1,5 года за 1$ закреп https://t.me/+Cz6_qO6kyCY5YTAy"
        ));
        assert!(!has_subscription_offer_bio(
            "Gemini нормально отвечает на вопросы, пробую бесплатную версию"
        ));
    }

    #[test]
    fn risk_score_is_capped_at_one_hundred() {
        let mut risk = RiskAccumulator::default();
        risk.add(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 120,
            label: "test",
            reason: "test",
        });
        assert_eq!(risk.finish(70).score, 100);
    }

    #[test]
    fn personal_channel_title_reused_by_spammers_is_strong() {
        let signal = personal_channel_title_reuse_signal(2, 2, 1)
            .expect("spammer reuse must produce a signal");
        assert_eq!(signal.coefficient, 24);
        assert_eq!(signal.label, "personal_channel_title_reused_by_spammers");
    }

    #[test]
    fn personal_channel_title_reused_by_new_accounts_is_supporting() {
        let signal = personal_channel_title_reuse_signal(0, 1, 1)
            .expect("new-account reuse must produce a signal");
        assert_eq!(signal.coefficient, 10);
    }

    #[test]
    fn personal_channel_title_without_reuse_is_silent() {
        assert!(personal_channel_title_reuse_signal(0, 0, 1).is_none());
        assert!(personal_channel_title_reuse_signal(0, 3, 10).is_none());
    }

    fn rotation_features(names: i64, usernames: i64) -> NewUserFeatures {
        NewUserFeatures {
            identity_display_name_count: names,
            identity_username_count: usernames,
            ..Default::default()
        }
    }

    #[test]
    fn identity_display_name_rotation_scores() {
        let signal = identity_rotation_signal(&rotation_features(2, 1))
            .expect("name rotation must produce a signal");
        assert_eq!(signal.coefficient, 12);
        assert_eq!(signal.label, "identity_display_name_rotation");
    }

    #[test]
    fn identity_username_rotation_scores_lower() {
        let signal = identity_rotation_signal(&rotation_features(1, 2))
            .expect("username rotation must produce a signal");
        assert_eq!(signal.coefficient, 8);
    }

    #[test]
    fn stable_identity_is_silent() {
        assert!(identity_rotation_signal(&rotation_features(1, 1)).is_none());
        assert!(identity_rotation_signal(&rotation_features(0, 0)).is_none());
    }
}
