use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportResolution {
    Pending,
    Accepted,
    Rejected,
}

impl ReportResolution {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReportCard {
    pub id: i64,
    pub chat_id: i64,
    pub message_id: i32,
    pub reporter_user_id: i64,
    pub reported_user_id: i64,
    pub reason: String,
    pub target_text: Option<String>,
    pub target_media: String,
    pub target_reply_to_message_id: Option<i32>,
    pub target_created_at: DateTime<Utc>,
    pub reporter_snapshot: Value,
    pub target_snapshot: Value,
    pub resolution: ReportResolution,
    pub profile_username: Option<String>,
    pub profile_photo_file_id: Option<String>,
    pub profile_first_name: Option<String>,
    pub profile_last_name: Option<String>,
    pub profile_is_bot: Option<bool>,
    pub profile_is_premium: Option<bool>,
    pub profile_language_code: Option<String>,
    pub profile_bio: Option<String>,
    pub personal_channel_title: Option<String>,
    pub personal_channel_username: Option<String>,
    pub personal_channel_has_adult_links: Option<bool>,
    pub first_seen_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub message_count: Option<i64>,
    pub reply_count: Option<i64>,
    pub link_count: Option<i64>,
    pub media_count: Option<i64>,
    pub is_spammer: Option<bool>,
    pub spam_score: Option<i32>,
    pub spam_type: Option<String>,
    pub spam_reason: Option<String>,
    pub spam_types: Option<Value>,
    pub spam_profile_labels: Option<Value>,
    pub member_status: Option<String>,
    pub is_admin: Option<bool>,
    pub is_present: Option<bool>,
    pub member_observed_at: Option<DateTime<Utc>>,
    pub audit_analyzed_at: Option<DateTime<Utc>>,
    pub audit_risk_score: Option<i32>,
    pub audit_risk_level: Option<String>,
    pub audit_primary_risk_class: Option<String>,
    pub audit_risk_labels: Option<Value>,
    pub audit_risk_reasons: Option<Value>,
}

pub async fn load_report(pool: &PgPool, report_id: i64) -> anyhow::Result<ReportCard> {
    let row = sqlx::query(
        r#"
        select
            report.id, report.chat_id, report.message_id,
            report.reporter_user_id, report.reported_user_id, report.reason,
            report.target_text, report.target_media,
            report.target_reply_to_message_id, report.target_created_at,
            report.reporter_snapshot, report.target_snapshot, report.resolution,
            profile.username as profile_username,
            profile.profile_photo_file_id,
            profile.first_name as profile_first_name,
            profile.last_name as profile_last_name,
            profile.is_bot as profile_is_bot,
            profile.is_premium as profile_is_premium,
            profile.language_code as profile_language_code,
            profile.bio as profile_bio,
            profile.personal_channel_title,
            profile.personal_channel_username,
            profile.personal_channel_has_adult_links,
            chat_user.first_seen_at,
            chat_user.last_seen_at,
            chat_user.message_count,
            chat_user.reply_count,
            chat_user.link_count,
            chat_user.media_count,
            chat_user.is_spammer,
            chat_user.spam_score,
            chat_user.spam_type,
            chat_user.spam_reason,
            chat_user.spam_types,
            chat_user.spam_profile_labels,
            chat_user.member_status,
            chat_user.is_admin,
            chat_user.is_present,
            chat_user.member_observed_at,
            audit.analyzed_at as audit_analyzed_at,
            audit.risk_score as audit_risk_score,
            audit.risk_level as audit_risk_level,
            audit.primary_risk_class as audit_primary_risk_class,
            audit.risk_labels as audit_risk_labels,
            audit.risk_reasons as audit_risk_reasons
        from telegram_reports report
        left join telegram_user_profiles profile
          on profile.telegram_user_id = report.reported_user_id
        left join telegram_chat_users chat_user
          on chat_user.chat_id = report.chat_id
         and chat_user.telegram_user_id = report.reported_user_id
        left join lateral (
            select analyzed_at, risk_score, risk_level, primary_risk_class,
                   risk_labels, risk_reasons
            from telegram_new_user_profile_audits
            where chat_id = report.chat_id
              and telegram_user_id = report.reported_user_id
            order by analyzed_at desc
            limit 1
        ) audit on true
        where report.id = $1
        "#,
    )
    .bind(report_id)
    .fetch_one(pool)
    .await?;

    let resolution = match row.get::<String, _>("resolution").as_str() {
        "accepted" => ReportResolution::Accepted,
        "rejected" => ReportResolution::Rejected,
        _ => ReportResolution::Pending,
    };
    Ok(ReportCard {
        id: row.get("id"),
        chat_id: row.get("chat_id"),
        message_id: row.get("message_id"),
        reporter_user_id: row.get("reporter_user_id"),
        reported_user_id: row.get("reported_user_id"),
        reason: row.get("reason"),
        target_text: row.get("target_text"),
        target_media: row.get("target_media"),
        target_reply_to_message_id: row.get("target_reply_to_message_id"),
        target_created_at: row.get("target_created_at"),
        reporter_snapshot: row.get("reporter_snapshot"),
        target_snapshot: row.get("target_snapshot"),
        resolution,
        profile_username: row.get("profile_username"),
        profile_photo_file_id: row.get("profile_photo_file_id"),
        profile_first_name: row.get("profile_first_name"),
        profile_last_name: row.get("profile_last_name"),
        profile_is_bot: row.get("profile_is_bot"),
        profile_is_premium: row.get("profile_is_premium"),
        profile_language_code: row.get("profile_language_code"),
        profile_bio: row.get("profile_bio"),
        personal_channel_title: row.get("personal_channel_title"),
        personal_channel_username: row.get("personal_channel_username"),
        personal_channel_has_adult_links: row.get("personal_channel_has_adult_links"),
        first_seen_at: row.get("first_seen_at"),
        last_seen_at: row.get("last_seen_at"),
        message_count: row.get("message_count"),
        reply_count: row.get("reply_count"),
        link_count: row.get("link_count"),
        media_count: row.get("media_count"),
        is_spammer: row.get("is_spammer"),
        spam_score: row.get("spam_score"),
        spam_type: row.get("spam_type"),
        spam_reason: row.get("spam_reason"),
        spam_types: row.get("spam_types"),
        spam_profile_labels: row.get("spam_profile_labels"),
        member_status: row.get("member_status"),
        is_admin: row.get("is_admin"),
        is_present: row.get("is_present"),
        member_observed_at: row.get("member_observed_at"),
        audit_analyzed_at: row.get("audit_analyzed_at"),
        audit_risk_score: row.get("audit_risk_score"),
        audit_risk_level: row.get("audit_risk_level"),
        audit_primary_risk_class: row.get("audit_primary_risk_class"),
        audit_risk_labels: row.get("audit_risk_labels"),
        audit_risk_reasons: row.get("audit_risk_reasons"),
    })
}
