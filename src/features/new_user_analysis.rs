use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};

#[derive(Debug, Clone, Copy)]
pub struct NewUserAnalysisConfig {
    pub recent_id_ratio_threshold: f64,
    pub old_user_message_threshold: i64,
}

impl Default for NewUserAnalysisConfig {
    fn default() -> Self {
        Self {
            recent_id_ratio_threshold: 0.92,
            old_user_message_threshold: 5,
        }
    }
}

#[derive(Debug, Clone)]
struct NewUserFeatures {
    chat_id: i64,
    telegram_user_id: i64,
    first_seen_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    last_seen_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    account_seen_age_sec: Option<i64>,
    chat_age_sec: Option<i64>,
    first_message_id: Option<i32>,
    last_message_id: Option<i32>,
    message_count: i64,
    reply_count: i64,
    link_count: i64,
    media_count: i64,
    voice_count: i64,
    reply_to_channel_post_count: i64,
    reply_to_bot_count: i64,
    top_level_message_count: i64,
    reply_to_comment_count: i64,
    message_count_24h: i64,
    link_count_24h: i64,
    burst_messages_per_min: Option<f64>,
    first_message_text: Option<String>,
    last_message_text: Option<String>,
    recent_message_texts: Vec<String>,
    text_texture: TextTexture,
    message_style: MessageStyle,
    id_rank_ratio: Option<f64>,
    username: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    display_name: Option<String>,
    display_name_reuse_count: i64,
    display_name_reuse_spammer_count: i64,
    is_bot: bool,
    is_premium: Option<bool>,
    language_code: Option<String>,
    bio: Option<String>,
    profile_photo_file_id: Option<String>,
    profile_photo_file_unique_id: Option<String>,
    profile_photo_count: Option<i32>,
    profile_photo_reuse_count: i64,
    profile_photo_width: Option<i32>,
    profile_photo_height: Option<i32>,
    emoji_status_custom_emoji_id: Option<String>,
    profile_accent_color_id: Option<i16>,
    personal_channel_chat_id: Option<i64>,
    personal_channel_title: Option<String>,
    personal_channel_username: Option<String>,
    personal_channel_message_count: Option<i32>,
    personal_channel_last_message_id: Option<i32>,
    personal_channel_last_message_at:
        Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    personal_channel_last_text: Option<String>,
    personal_channel_has_adult_links: bool,
    personal_channel_refreshed_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    personal_channel_fetch_error: Option<String>,
    member_status: Option<String>,
    member_is_present: Option<bool>,
    member_is_admin: Option<bool>,
    join_event_seen: bool,
    invite_link: Option<String>,
    via_chat_folder_invite_link: bool,
}

#[derive(Debug, Clone, Default)]
struct TextTexture {
    normalized_count: i64,
    distinct_normalized_count: i64,
    duplicate_normalized_count: i64,
    max_reuse_count: i64,
    max_pairwise_similarity: Option<f64>,
    avg_message_len: Option<f64>,
    repetitive_pattern: bool,
}

#[derive(Debug, Clone, Default)]
struct MessageStyle {
    text_message_count: i64,
    single_exclamation_ending_count: i64,
    period_ending_count: i64,
    emoji_message_count: i64,
    emoji_ending_count: i64,
}

#[derive(Debug, Clone)]
struct RiskAnalysis {
    score: i32,
    level: String,
    primary_class: Option<String>,
    class_scores: Value,
    labels: Vec<String>,
    reasons: Vec<String>,
    signals: Value,
}

#[derive(Debug, Clone, Copy)]
enum WarningStrength {
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
enum SpamClass {
    AdultPersonalChannel,
    ForeignInviteLink,
    LlmProfileBait,
    PromoDmBait,
    LinkDropper,
    FreshAccount,
}

impl SpamClass {
    fn all() -> [Self; 6] {
        [
            Self::AdultPersonalChannel,
            Self::ForeignInviteLink,
            Self::LlmProfileBait,
            Self::PromoDmBait,
            Self::LinkDropper,
            Self::FreshAccount,
        ]
    }

    fn as_str(self) -> &'static str {
        match self {
            SpamClass::AdultPersonalChannel => "adult_personal_channel_promo",
            SpamClass::ForeignInviteLink => "foreign_invite_link_spam",
            SpamClass::LlmProfileBait => "llm_profile_bait",
            SpamClass::PromoDmBait => "promo_dm_bait",
            SpamClass::LinkDropper => "link_dropper",
            SpamClass::FreshAccount => "fresh_account_risk",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RiskSignal {
    class: SpamClass,
    coefficient: i32,
    label: &'static str,
    reason: &'static str,
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

    fn finish(mut self) -> RiskAnalysis {
        let level = match self.score {
            70.. => "high",
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

pub async fn analyze_new_user_profile(
    pool: &PgPool,
    chat_id: i64,
    telegram_user_id: i64,
) -> anyhow::Result<()> {
    analyze_new_user_profile_with_config(
        pool,
        chat_id,
        telegram_user_id,
        NewUserAnalysisConfig::default(),
    )
    .await
}

pub async fn analyze_new_user_profile_with_config(
    pool: &PgPool,
    chat_id: i64,
    telegram_user_id: i64,
    config: NewUserAnalysisConfig,
) -> anyhow::Result<()> {
    let Some(features) = load_features(pool, chat_id, telegram_user_id).await? else {
        return Ok(());
    };

    // Старые активные пользователи не цель этой системы: сохраняем low-risk snapshot,
    // но не накидываем профильные штрафы за нормальное накопленное поведение.
    let is_old_active_user = features.message_count >= config.old_user_message_threshold;
    let risk = analyze_risk(&features, &config, is_old_active_user);
    tracing::info!(
        chat_id,
        telegram_user_id,
        risk_score = risk.score,
        risk_level = %risk.level,
        risk_signals = %risk.signals,
        "new user spam risk analyzed"
    );
    save_audit(pool, &features, &risk, &config).await
}

async fn load_features(
    pool: &PgPool,
    chat_id: i64,
    telegram_user_id: i64,
) -> anyhow::Result<Option<NewUserFeatures>> {
    let row = sqlx::query(
        r#"
        with user_messages as (
            select *
            from telegram_messages
            where chat_id = $1
              and user_id = $2
              and source_channel_id is null
        ), first_msg as (
            select message_id, text
            from user_messages
            order by created_at asc
            limit 1
        ), last_msg as (
            select message_id, text
            from user_messages
            order by created_at desc
            limit 1
        ), normalized_messages as (
            select
                message_id,
                text,
                created_at,
                nullif(regexp_replace(lower(coalesce(text, '')), '\\s+', ' ', 'g'), '') as normalized_text
            from user_messages
        ), normalized_counts as (
            select normalized_text, count(*)::bigint as reuse_count
            from normalized_messages
            where normalized_text is not null
            group by normalized_text
        ), msg_stats as (
            select
                count(*)::bigint as message_count,
                count(*) filter (where reply_to_message_id is not null)::bigint as reply_count,
                count(*) filter (where has_links)::bigint as link_count,
                count(*) filter (where has_photo or has_video or has_document or has_audio or has_voice or has_sticker or has_animation)::bigint as media_count,
                count(*) filter (where has_voice)::bigint as voice_count,
                count(*) filter (where reply_to_message_id in (select discussion_message_id from post_comment_jobs where discussion_chat_id = $1))::bigint as reply_to_channel_post_count,
                count(*) filter (where reply_to_message_id in (select bot_comment_message_id from post_comment_jobs where discussion_chat_id = $1))::bigint as reply_to_bot_count,
                count(*) filter (where reply_to_message_id is null)::bigint as top_level_message_count,
                count(*) filter (
                    where reply_to_message_id is not null
                      and reply_to_message_id not in (select discussion_message_id from post_comment_jobs where discussion_chat_id = $1)
                      and reply_to_message_id not in (select bot_comment_message_id from post_comment_jobs where discussion_chat_id = $1)
                )::bigint as reply_to_comment_count,
                count(*) filter (where created_at >= now() - interval '24 hours')::bigint as message_count_24h,
                count(*) filter (where has_links and created_at >= now() - interval '24 hours')::bigint as link_count_24h,
                avg(char_length(coalesce(text, '')))::double precision as avg_message_len,
                array_remove(array_agg(text order by created_at desc, message_id desc), null) as recent_message_texts
            from user_messages
        ), texture_stats as (
            select
                coalesce(sum(reuse_count), 0)::bigint as normalized_message_count,
                count(normalized_text)::bigint as distinct_normalized_message_count,
                coalesce(sum(greatest(reuse_count - 1, 0)), 0)::bigint as duplicate_normalized_message_count,
                coalesce(max(reuse_count), 0)::bigint as max_normalized_message_reuse_count
            from normalized_counts
        ), id_rank as (
            select
                case
                    when max(telegram_user_id) = min(telegram_user_id) then 1.0::double precision
                    else (($2::double precision - min(telegram_user_id)::double precision)
                        / nullif(max(telegram_user_id)::double precision - min(telegram_user_id)::double precision, 0))
                end as ratio
            from telegram_user_profiles
            where telegram_user_id > 0 and not coalesce(is_bot, false)
        ), latest_join_event as (
            select invite_link, via_chat_folder_invite_link
            from telegram_chat_member_events
            where chat_id = $1 and telegram_user_id = $2
            order by event_at desc
            limit 1
        )
        select
            cu.chat_id,
            cu.telegram_user_id,
            cu.first_seen_at,
            cu.last_seen_at,
            extract(epoch from (coalesce(cu.last_seen_at, now()) - cu.first_seen_at))::bigint as account_seen_age_sec,
            extract(epoch from (now() - cu.first_seen_at))::bigint as chat_age_sec,
            cu.first_message_id,
            cu.last_message_id,
            coalesce(ms.message_count, cu.message_count, 0)::bigint as message_count,
            coalesce(ms.reply_count, cu.reply_count, 0)::bigint as reply_count,
            coalesce(ms.link_count, cu.link_count, 0)::bigint as link_count,
            coalesce(ms.media_count, cu.media_count, 0)::bigint as media_count,
            coalesce(ms.voice_count, 0)::bigint as voice_count,
            coalesce(ms.reply_to_channel_post_count, cu.reply_to_channel_post_count, 0)::bigint as reply_to_channel_post_count,
            coalesce(ms.reply_to_bot_count, cu.reply_to_bot_count, 0)::bigint as reply_to_bot_count,
            coalesce(ms.top_level_message_count, 0)::bigint as top_level_message_count,
            coalesce(ms.reply_to_comment_count, 0)::bigint as reply_to_comment_count,
            coalesce(ms.message_count_24h, 0)::bigint as message_count_24h,
            coalesce(ms.link_count_24h, 0)::bigint as link_count_24h,
            case
                when cu.first_seen_at is null or cu.last_seen_at is null then null
                when extract(epoch from (cu.last_seen_at - cu.first_seen_at)) <= 0 then null
                else (coalesce(ms.message_count, cu.message_count, 0)::double precision / greatest(extract(epoch from (cu.last_seen_at - cu.first_seen_at)) / 60.0, 1.0))
            end as burst_messages_per_min,
            fm.message_id as first_message_id_from_messages,
            fm.text as first_message_text,
            lm.message_id as last_message_id_from_messages,
            lm.text as last_message_text,
            coalesce(ms.recent_message_texts, array[]::text[]) as recent_message_texts,
            coalesce(ts.normalized_message_count, 0)::bigint as normalized_message_count,
            coalesce(ts.distinct_normalized_message_count, 0)::bigint as distinct_normalized_message_count,
            coalesce(ts.duplicate_normalized_message_count, 0)::bigint as duplicate_normalized_message_count,
            coalesce(ts.max_normalized_message_reuse_count, 0)::bigint as max_normalized_message_reuse_count,
            ms.avg_message_len,
            ir.ratio as id_rank_ratio,
            p.username,
            p.first_name,
            p.last_name,
            nullif(trim(concat_ws(' ', p.first_name, p.last_name)), '') as display_name,
            coalesce(dns.reuse_count, 0)::bigint as display_name_reuse_count,
            coalesce(dns.reuse_spammer_count, 0)::bigint as display_name_reuse_spammer_count,
            coalesce(p.is_bot, false) as is_bot,
            p.is_premium,
            p.language_code,
            p.bio,
            p.profile_photo_file_id,
            p.profile_photo_file_unique_id,
            p.profile_photo_count,
            coalesce(pr.reuse_count, 0)::bigint as profile_photo_reuse_count,
            p.profile_photo_width,
            p.profile_photo_height,
            p.emoji_status_custom_emoji_id,
            p.profile_accent_color_id,
            p.personal_channel_chat_id,
            p.personal_channel_title,
            p.personal_channel_username,
            p.personal_channel_message_count,
            p.personal_channel_last_message_id,
            p.personal_channel_last_message_at,
            p.personal_channel_last_text,
            coalesce(p.personal_channel_has_adult_links, false) as personal_channel_has_adult_links,
            p.personal_channel_refreshed_at,
            p.personal_channel_fetch_error,
            s.status as member_status,
            s.is_present as member_is_present,
            s.is_admin as member_is_admin,
            exists (
                select 1
                from telegram_chat_member_events e
                where e.chat_id = $1 and e.telegram_user_id = $2
            ) as join_event_seen,
            lje.invite_link,
            coalesce(lje.via_chat_folder_invite_link, false) as via_chat_folder_invite_link
        from telegram_chat_users cu
        left join telegram_user_profiles p on p.telegram_user_id = cu.telegram_user_id
        left join telegram_chat_member_snapshots s on s.chat_id = cu.chat_id and s.telegram_user_id = cu.telegram_user_id
        left join msg_stats ms on true
        left join first_msg fm on true
        left join last_msg lm on true
        left join texture_stats ts on true
        left join id_rank ir on true
        left join latest_join_event lje on true
        left join lateral (
            select
                count(*)::bigint as reuse_count,
                count(*) filter (where coalesce(cu2.is_spammer, false))::bigint as reuse_spammer_count
            from telegram_user_profiles p2
            left join telegram_chat_users cu2
              on cu2.chat_id = $1 and cu2.telegram_user_id = p2.telegram_user_id
            where nullif(trim(concat_ws(' ', p.first_name, p.last_name)), '') is not null
              and lower(nullif(trim(concat_ws(' ', p2.first_name, p2.last_name)), '')) = lower(nullif(trim(concat_ws(' ', p.first_name, p.last_name)), ''))
              and p2.telegram_user_id <> p.telegram_user_id
        ) dns on true
        left join lateral (
            select count(*)::bigint as reuse_count
            from telegram_user_profiles p2
            where p.profile_photo_file_unique_id is not null
              and p2.profile_photo_file_unique_id = p.profile_photo_file_unique_id
              and p2.telegram_user_id <> p.telegram_user_id
        ) pr on true
        where cu.chat_id = $1 and cu.telegram_user_id = $2
        "#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| {
        let recent_message_texts = row.get::<Vec<String>, _>("recent_message_texts");
        let message_style = message_style(&recent_message_texts);
        let max_pairwise_similarity = max_pairwise_message_similarity(&recent_message_texts);
        let max_reuse_count = row.get("max_normalized_message_reuse_count");
        let duplicate_normalized_message_count = row.get("duplicate_normalized_message_count");
        let repetitive_pattern = duplicate_normalized_message_count > 0
            || max_reuse_count > 1
            || max_pairwise_similarity.is_some_and(|similarity| similarity >= 0.86);

        NewUserFeatures {
            chat_id: row.get("chat_id"),
            telegram_user_id: row.get("telegram_user_id"),
            first_seen_at: row.get("first_seen_at"),
            last_seen_at: row.get("last_seen_at"),
            account_seen_age_sec: row.get("account_seen_age_sec"),
            chat_age_sec: row.get("chat_age_sec"),
            first_message_id: row
                .try_get("first_message_id_from_messages")
                .ok()
                .or_else(|| row.try_get("first_message_id").ok()),
            last_message_id: row
                .try_get("last_message_id_from_messages")
                .ok()
                .or_else(|| row.try_get("last_message_id").ok()),
            message_count: row.get("message_count"),
            reply_count: row.get("reply_count"),
            link_count: row.get("link_count"),
            media_count: row.get("media_count"),
            voice_count: row.get("voice_count"),
            reply_to_channel_post_count: row.get("reply_to_channel_post_count"),
            reply_to_bot_count: row.get("reply_to_bot_count"),
            top_level_message_count: row.get("top_level_message_count"),
            reply_to_comment_count: row.get("reply_to_comment_count"),
            message_count_24h: row.get("message_count_24h"),
            link_count_24h: row.get("link_count_24h"),
            burst_messages_per_min: row.get("burst_messages_per_min"),
            first_message_text: row.get("first_message_text"),
            last_message_text: row.get("last_message_text"),
            recent_message_texts,
            text_texture: TextTexture {
                normalized_count: row.get("normalized_message_count"),
                distinct_normalized_count: row.get("distinct_normalized_message_count"),
                duplicate_normalized_count: duplicate_normalized_message_count,
                max_reuse_count,
                max_pairwise_similarity,
                avg_message_len: row.get("avg_message_len"),
                repetitive_pattern,
            },
            message_style,
            id_rank_ratio: row.get("id_rank_ratio"),
            username: row.get("username"),
            first_name: row.get("first_name"),
            last_name: row.get("last_name"),
            display_name: row.get("display_name"),
            display_name_reuse_count: row.get("display_name_reuse_count"),
            display_name_reuse_spammer_count: row.get("display_name_reuse_spammer_count"),
            is_bot: row.get("is_bot"),
            is_premium: row.get("is_premium"),
            language_code: row.get("language_code"),
            bio: row.get("bio"),
            profile_photo_file_id: row.get("profile_photo_file_id"),
            profile_photo_file_unique_id: row.get("profile_photo_file_unique_id"),
            profile_photo_count: row.get("profile_photo_count"),
            profile_photo_reuse_count: row.get("profile_photo_reuse_count"),
            profile_photo_width: row.get("profile_photo_width"),
            profile_photo_height: row.get("profile_photo_height"),
            emoji_status_custom_emoji_id: row.get("emoji_status_custom_emoji_id"),
            profile_accent_color_id: row.get("profile_accent_color_id"),
            personal_channel_chat_id: row.get("personal_channel_chat_id"),
            personal_channel_title: row.get("personal_channel_title"),
            personal_channel_username: row.get("personal_channel_username"),
            personal_channel_message_count: row.get("personal_channel_message_count"),
            personal_channel_last_message_id: row.get("personal_channel_last_message_id"),
            personal_channel_last_message_at: row.get("personal_channel_last_message_at"),
            personal_channel_last_text: row.get("personal_channel_last_text"),
            personal_channel_has_adult_links: row.get("personal_channel_has_adult_links"),
            personal_channel_refreshed_at: row.get("personal_channel_refreshed_at"),
            personal_channel_fetch_error: row.get("personal_channel_fetch_error"),
            member_status: row.get("member_status"),
            member_is_present: row.get("member_is_present"),
            member_is_admin: row.get("member_is_admin"),
            join_event_seen: row.get("join_event_seen"),
            invite_link: row.get("invite_link"),
            via_chat_folder_invite_link: row.get("via_chat_folder_invite_link"),
        }
    }))
}

fn analyze_risk(
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

fn analyze_new_or_low_activity_user(
    features: &NewUserFeatures,
    config: &NewUserAnalysisConfig,
) -> RiskAnalysis {
    let username_stats = username_stats(features.username.as_deref());
    let mut risk = RiskAccumulator::default();

    risk.add_optional(message_count_signal(features));
    risk.add_optional(link_signal(features));
    risk.add_optional(foreign_invite_link_signal(features));
    risk.add_optional(recent_id_signal(features, config));
    risk.add_optional(username_signal(&username_stats));
    risk.add_optional(display_name_signal(features));
    risk.add_optional(profile_photo_signal(features));
    risk.add_optional(feminine_name_signal(features));
    risk.add_optional(message_texture_signal(features));
    risk.add_optional(chat_position_signal(features));
    for signal in generic_feminine_message_style_signals(features) {
        risk.add(signal);
    }
    risk.add_optional(reply_to_comment_mitigation_signal(features));
    risk.add_optional(chat_age_signal(features));

    for signal in personal_channel_signals(features) {
        risk.add(signal);
    }

    risk.add_optional(short_bio_signal(features));
    risk.add_optional(member_status_signal(features));
    risk.finish()
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
    match features
        .id_rank_ratio
        .is_some_and(|ratio| ratio >= config.recent_id_ratio_threshold)
    {
        true => Some(RiskSignal {
            class: SpamClass::FreshAccount,
            coefficient: 15,
            label: "recent_high_telegram_id",
            reason: "Telegram user id is in the recent high-id range observed by the bot",
        }),
        false => None,
    }
}

fn username_signal(stats: &UsernameStats) -> Option<RiskSignal> {
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
        (count, 0, channel_comments, bot_replies, comment_replies)
            if count > 0 && channel_comments + bot_replies + comment_replies >= count =>
        {
            Some(RiskSignal {
                class: SpamClass::LlmProfileBait,
                coefficient: 9,
                label: "only_replies_or_comments",
                reason: "New user appears only in comment/reply contexts, not as normal chat participant",
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

fn generic_feminine_message_style_signals(features: &NewUserFeatures) -> Vec<RiskSignal> {
    if !looks_like_feminine_first_name(features.first_name.as_deref()) || features.message_count > 5
    {
        return Vec::new();
    }

    let style = &features.message_style;
    let mut signals = Vec::new();

    if style.single_exclamation_ending_count > 0 {
        signals.push(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 3,
            label: "generic_feminine_single_exclamation_ending",
            reason: "Generic-feminine persona message ends with exactly one exclamation mark",
        });
    }
    if features.reply_to_channel_post_count > 0 && features.reply_to_comment_count == 0 {
        signals.push(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 6,
            label: "generic_feminine_reply_to_channel_post",
            reason: "Generic-feminine persona replies directly to a channel post, not another comment",
        });
    }
    if style.emoji_message_count > 0 {
        signals.push(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 2,
            label: "generic_feminine_message_has_emoji",
            reason: "Generic-feminine persona uses emoji in a new-user message",
        });
    }
    if style.emoji_ending_count > 0 {
        signals.push(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 3,
            label: "generic_feminine_message_ends_with_emoji",
            reason: "Generic-feminine persona message ends with emoji",
        });
    }
    if style.period_ending_count > 0 {
        signals.push(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 1,
            label: "generic_feminine_single_period_ending",
            reason: "Generic-feminine persona message ends with exactly one period",
        });
    }

    signals
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

fn only_replies_or_comments(features: &NewUserFeatures) -> bool {
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

fn only_channel_post_comments(features: &NewUserFeatures) -> bool {
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

    if let Some(channel_id) = features.personal_channel_chat_id {
        signals.push(RiskSignal {
            class: SpamClass::LlmProfileBait,
            coefficient: 12,
            label: "personal_channel_attached",
            reason: "User has an attached personal channel",
        });
        if channel_id.abs() > 4_000_000_000_000 {
            signals.push(RiskSignal {
                class: SpamClass::FreshAccount,
                coefficient: 6,
                label: "recent_personal_channel_id",
                reason: "Attached personal channel id is in a very high range",
            });
        }
    }

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

    signals
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

fn audit_insert_columns() -> &'static [&'static str] {
    &[
        "chat_id",
        "telegram_user_id",
        "first_seen_at",
        "last_seen_at",
        "account_seen_age_sec",
        "chat_age_sec",
        "first_message_id",
        "last_message_id",
        "message_count",
        "reply_count",
        "link_count",
        "media_count",
        "voice_count",
        "reply_to_channel_post_count",
        "reply_to_bot_count",
        "top_level_message_count",
        "reply_to_comment_count",
        "only_replies_or_comments",
        "only_channel_post_comments",
        "message_count_24h",
        "link_count_24h",
        "burst_messages_per_min",
        "first_message_text",
        "first_message_len",
        "last_message_text",
        "last_message_len",
        "normalized_message_count",
        "distinct_normalized_message_count",
        "duplicate_normalized_message_count",
        "max_normalized_message_reuse_count",
        "max_pairwise_message_similarity",
        "avg_message_len",
        "repetitive_message_pattern",
        "telegram_user_id_bucket",
        "telegram_user_id_rank_ratio",
        "telegram_user_id_is_recent",
        "username",
        "username_len",
        "username_has_digits",
        "username_digit_count",
        "username_has_random_suffix",
        "username_pattern",
        "first_name",
        "last_name",
        "display_name",
        "first_name_feminine_pattern",
        "display_name_reuse_count",
        "display_name_reuse_spammer_count",
        "display_name_reused_by_spammers",
        "is_bot",
        "is_premium",
        "language_code",
        "bio",
        "bio_len",
        "has_profile_photo",
        "profile_photo_count",
        "profile_photo_reuse_count",
        "profile_photo_file_unique_id",
        "profile_photo_dc_id",
        "profile_photo_dc_source",
        "profile_photo_width",
        "profile_photo_height",
        "has_emoji_status",
        "profile_accent_color_id",
        "personal_channel_chat_id",
        "personal_channel_title",
        "personal_channel_username",
        "personal_channel_message_count",
        "personal_channel_last_message_id",
        "personal_channel_last_message_at",
        "personal_channel_last_text",
        "personal_channel_has_adult_links",
        "personal_channel_has_invite_link",
        "personal_channel_has_external_link",
        "personal_channel_title_len",
        "personal_channel_last_text_len",
        "personal_channel_refreshed_at",
        "personal_channel_fetch_error",
        "member_status",
        "member_is_present",
        "member_is_admin",
        "join_event_seen",
        "invite_link",
        "via_chat_folder_invite_link",
        "risk_score",
        "risk_level",
        "primary_risk_class",
        "risk_class_scores",
        "risk_labels",
        "risk_reasons",
        "risk_signal_breakdown",
        "raw_features",
    ]
}

async fn save_audit(
    pool: &PgPool,
    features: &NewUserFeatures,
    risk: &RiskAnalysis,
    config: &NewUserAnalysisConfig,
) -> anyhow::Result<()> {
    let username_stats = username_stats(features.username.as_deref());
    let profile_photo_dc = best_effort_profile_photo_dc(features.profile_photo_file_id.as_deref());
    let first_message_len = features.first_message_text.as_deref().map(char_count_i32);
    let last_message_len = features.last_message_text.as_deref().map(char_count_i32);
    let bio_len = features.bio.as_deref().map(char_count_i32);
    let channel_title_len = features
        .personal_channel_title
        .as_deref()
        .map(char_count_i32);
    let channel_last_text_len = features
        .personal_channel_last_text
        .as_deref()
        .map(char_count_i32);
    let raw_features = json!({
        "dc": {
            "available": profile_photo_dc.dc_id.is_some(),
            "source": profile_photo_dc.source,
            "note": profile_photo_dc.note,
        },
        "thresholds": {
            "recent_id_ratio": config.recent_id_ratio_threshold,
            "old_user_message_threshold": config.old_user_message_threshold,
        },
        "known_risk_classes": SpamClass::all().map(SpamClass::as_str),
        "profile_photo_file_id_present": features.profile_photo_file_id.is_some(),
        "profile_photo_file_unique_id_present": features.profile_photo_file_unique_id.is_some(),
        "profile_photo_reuse_count": features.profile_photo_reuse_count,
        "first_name_feminine_pattern": looks_like_feminine_first_name(features.first_name.as_deref()),
        "chat_context": {
            "only_replies_or_comments": only_replies_or_comments(features),
            "only_channel_post_comments": only_channel_post_comments(features),
            "reply_to_channel_post_count": features.reply_to_channel_post_count,
            "reply_to_bot_count": features.reply_to_bot_count,
            "reply_to_comment_count": features.reply_to_comment_count,
            "top_level_message_count": features.top_level_message_count,
        },
        "text_texture": {
            "recent_message_text_count": features.recent_message_texts.len(),
            "normalized_message_count": features.text_texture.normalized_count,
            "distinct_normalized_message_count": features.text_texture.distinct_normalized_count,
            "duplicate_normalized_message_count": features.text_texture.duplicate_normalized_count,
            "max_normalized_message_reuse_count": features.text_texture.max_reuse_count,
            "max_pairwise_message_similarity": features.text_texture.max_pairwise_similarity,
            "repetitive_message_pattern": features.text_texture.repetitive_pattern,
        },
        "message_style": {
            "text_message_count": features.message_style.text_message_count,
            "single_exclamation_ending_count": features.message_style.single_exclamation_ending_count,
            "period_ending_count": features.message_style.period_ending_count,
            "emoji_message_count": features.message_style.emoji_message_count,
            "emoji_ending_count": features.message_style.emoji_ending_count,
        },
    });

    let columns = audit_insert_columns();
    let mut query = QueryBuilder::<Postgres>::new("insert into telegram_new_user_profile_audits (");

    {
        let mut separated = query.separated(", ");
        for column in columns {
            separated.push(*column);
        }
    }

    query.push(") values (");
    {
        let mut values = query.separated(", ");
        values.push_bind(features.chat_id);
        values.push_bind(features.telegram_user_id);
        values.push_bind(features.first_seen_at);
        values.push_bind(features.last_seen_at);
        values.push_bind(features.account_seen_age_sec);
        values.push_bind(features.chat_age_sec);
        values.push_bind(features.first_message_id);
        values.push_bind(features.last_message_id);
        values.push_bind(features.message_count);
        values.push_bind(features.reply_count);
        values.push_bind(features.link_count);
        values.push_bind(features.media_count);
        values.push_bind(features.voice_count);
        values.push_bind(features.reply_to_channel_post_count);
        values.push_bind(features.reply_to_bot_count);
        values.push_bind(features.top_level_message_count);
        values.push_bind(features.reply_to_comment_count);
        values.push_bind(only_replies_or_comments(features));
        values.push_bind(only_channel_post_comments(features));
        values.push_bind(features.message_count_24h);
        values.push_bind(features.link_count_24h);
        values.push_bind(features.burst_messages_per_min);
        values.push_bind(&features.first_message_text);
        values.push_bind(first_message_len);
        values.push_bind(&features.last_message_text);
        values.push_bind(last_message_len);
        values.push_bind(features.text_texture.normalized_count);
        values.push_bind(features.text_texture.distinct_normalized_count);
        values.push_bind(features.text_texture.duplicate_normalized_count);
        values.push_bind(features.text_texture.max_reuse_count);
        values.push_bind(features.text_texture.max_pairwise_similarity);
        values.push_bind(features.text_texture.avg_message_len);
        values.push_bind(features.text_texture.repetitive_pattern);
        values.push_bind(id_bucket(features.telegram_user_id));
        values.push_bind(features.id_rank_ratio);
        values.push_bind(
            features
                .id_rank_ratio
                .is_some_and(|ratio| ratio >= config.recent_id_ratio_threshold),
        );
        values.push_bind(&features.username);
        values.push_bind(features.username.as_deref().map(char_count_i32));
        values.push_bind(username_stats.has_digits);
        values.push_bind(username_stats.digit_count);
        values.push_bind(username_stats.has_random_suffix);
        values.push_bind(&username_stats.pattern);
        values.push_bind(&features.first_name);
        values.push_bind(&features.last_name);
        values.push_bind(&features.display_name);
        values.push_bind(looks_like_feminine_first_name(
            features.first_name.as_deref(),
        ));
        values.push_bind(features.display_name_reuse_count);
        values.push_bind(features.display_name_reuse_spammer_count);
        values.push_bind(features.display_name_reuse_spammer_count > 0);
        values.push_bind(features.is_bot);
        values.push_bind(features.is_premium);
        values.push_bind(&features.language_code);
        values.push_bind(&features.bio);
        values.push_bind(bio_len);
        values.push_bind(has_profile_photo(features));
        values.push_bind(features.profile_photo_count);
        values.push_bind(features.profile_photo_reuse_count);
        values.push_bind(&features.profile_photo_file_unique_id);
        values.push_bind(profile_photo_dc.dc_id);
        values.push_bind(&profile_photo_dc.source);
        values.push_bind(features.profile_photo_width);
        values.push_bind(features.profile_photo_height);
        values.push_bind(features.emoji_status_custom_emoji_id.is_some());
        values.push_bind(features.profile_accent_color_id);
        values.push_bind(features.personal_channel_chat_id);
        values.push_bind(&features.personal_channel_title);
        values.push_bind(&features.personal_channel_username);
        values.push_bind(features.personal_channel_message_count);
        values.push_bind(features.personal_channel_last_message_id);
        values.push_bind(features.personal_channel_last_message_at);
        values.push_bind(&features.personal_channel_last_text);
        values.push_bind(features.personal_channel_has_adult_links);
        values.push_bind(personal_channel_has_invite_link(features));
        values.push_bind(personal_channel_has_external_link(features));
        values.push_bind(channel_title_len);
        values.push_bind(channel_last_text_len);
        values.push_bind(features.personal_channel_refreshed_at);
        values.push_bind(&features.personal_channel_fetch_error);
        values.push_bind(&features.member_status);
        values.push_bind(features.member_is_present);
        values.push_bind(features.member_is_admin);
        values.push_bind(features.join_event_seen);
        values.push_bind(&features.invite_link);
        values.push_bind(features.via_chat_folder_invite_link);
        values.push_bind(risk.score);
        values.push_bind(&risk.level);
        values.push_bind(&risk.primary_class);
        values.push_bind(&risk.class_scores);
        values.push_bind(json!(risk.labels));
        values.push_bind(json!(risk.reasons));
        values.push_bind(&risk.signals);
        values.push_bind(raw_features);
    }

    query.push(") on conflict (chat_id, telegram_user_id) do update set analyzed_at = now(), ");
    {
        let mut updates = query.separated(", ");
        for column in columns
            .iter()
            .copied()
            .filter(|column| !matches!(*column, "chat_id" | "telegram_user_id"))
        {
            updates.push(format_args!("{column} = excluded.{column}"));
        }
    }

    query.build().execute(pool).await?;

    Ok(())
}

#[derive(Debug, Clone)]
struct UsernameStats {
    has_digits: bool,
    digit_count: i32,
    has_random_suffix: bool,
    pattern: String,
}

fn username_stats(username: Option<&str>) -> UsernameStats {
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

fn looks_like_feminine_first_name(first_name: Option<&str>) -> bool {
    let Some(first_name) = first_name.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let normalized = first_name
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
            | "полина"
            | "светлана"
            | "софия"
            | "таисия"
            | "татьяна"
            | "ульяна"
            | "юлия"
            | "яна"
    )
}

fn has_profile_photo(features: &NewUserFeatures) -> bool {
    features.profile_photo_file_unique_id.is_some()
        || features.profile_photo_file_id.is_some()
        || features.profile_photo_count.unwrap_or_default() > 0
}

fn personal_channel_has_invite_link(features: &NewUserFeatures) -> bool {
    let text = personal_channel_text_blob(features).to_lowercase();
    text.contains("t.me/+") || text.contains("telegram.me/+")
}

fn personal_channel_has_external_link(features: &NewUserFeatures) -> bool {
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

fn message_style(texts: &[String]) -> MessageStyle {
    texts
        .iter()
        .fold(MessageStyle::default(), |mut style, text| {
            let text = text.trim_end();
            if text.is_empty() {
                return style;
            }

            style.text_message_count += 1;
            if trailing_char_count(text, '!') == 1 {
                style.single_exclamation_ending_count += 1;
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

fn max_pairwise_message_similarity(texts: &[String]) -> Option<f64> {
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

fn char_count_i32(value: &str) -> i32 {
    i32::try_from(value.chars().count()).unwrap_or(i32::MAX)
}

fn id_bucket(user_id: i64) -> String {
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

#[derive(Debug, Clone)]
struct DcParseResult {
    dc_id: Option<i32>,
    source: Option<String>,
    note: String,
}

fn best_effort_profile_photo_dc(file_id: Option<&str>) -> DcParseResult {
    let Some(file_id) = file_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return DcParseResult {
            dc_id: None,
            source: None,
            note: "no_profile_photo_file_id".to_string(),
        };
    };

    // Telegram Bot API does not expose DC directly. Desktop clients such as AyuGram
    // can show it because they decode MTProto file locations. Bot API file_id is an
    // opaque, versioned identifier; guessing a DC from random bytes would create bad
    // training labels. Keep decoded metadata only as a future hook for a verified
    // Pyrogram/AyuGram-compatible decoder.
    let normalized = file_id.replace('-', "+").replace('_', "/");
    let decode_attempt = URL_SAFE_NO_PAD
        .decode(file_id.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(normalized.as_bytes()));
    let note = match decode_attempt {
        Ok(bytes) => format!(
            "dc_unavailable_from_bot_api; file_id_decoded_bytes={} but no verified decoder is installed",
            bytes.len()
        ),
        Err(_) => "dc_unavailable_from_bot_api; file_id_decode_failed".to_string(),
    };

    DcParseResult {
        dc_id: None,
        source: Some("bot_api_file_id_unverified".to_string()),
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let signals = risk.finish().signals;

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
    fn dc_parser_does_not_guess_without_verified_decoder() {
        let result = best_effort_profile_photo_dc(Some("AQADBAADb6sxG4x8AAEC"));
        assert!(result.dc_id.is_none());
        assert!(result.note.contains("dc_unavailable_from_bot_api"));
    }
}
