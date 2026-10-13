use chrono::{DateTime, Utc};

use crate::telegram::html::Html;

#[derive(Clone, Copy)]
pub enum StatsPeriod {
    Day,
    Week,
    Month,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatsRender {
    Html,
    Rich,
}

/// Fixed bounds shared by every query of one period report.
///
/// The repository computes the editorial 05:00 Moscow start; callers bind both
/// timestamps rather than independently evaluating `now()` in each query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReportWindow {
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub chat_day_index: i64,
}

impl ReportWindow {
    pub const fn new(start_at: DateTime<Utc>, end_at: DateTime<Utc>, chat_day_index: i64) -> Self {
        Self {
            start_at,
            end_at,
            chat_day_index,
        }
    }
}

#[derive(Clone)]
pub struct ChatStatsSummary {
    pub start_at: DateTime<Utc>,
    /// User-authored discussion messages in the report window.
    pub messages: i64,
    /// Distinct human authors of messages in the report window.
    pub active_users: i64,
    /// Human messages that reply to another message.
    pub replies: i64,
    /// Human messages containing one or more links.
    pub links: i64,
    /// Human messages with any supported media type; a message counts once.
    pub media: i64,
    pub channel_posts: i64,
    pub bot_comments: i64,
    /// Human messages in the report window replying directly to any sent bot comment.
    pub replies_to_bot: i64,
    /// Added reaction identities in event updates; removals do not decrement this.
    pub reaction_additions: i64,
    /// Messages whose latest stored reaction-count snapshot was updated in the window.
    pub messages_with_reaction_count_update: i64,
    /// Sum of latest stored reaction counts on bot comments created in the window.
    pub bot_comment_reactions: i64,
    /// Present-state transitions, including `restricted.is_member` changes.
    pub joins: i64,
    pub leaves: i64,
}

/// Complete period data shared by the Telegram HTML and Rich renderers.
/// Renderer modules only format this model; SQL lives in `stats::repo`.
#[derive(Clone)]
pub struct ChatStatsReportData {
    pub period: StatsPeriod,
    pub summary: ChatStatsSummary,
    pub attraction: AttractionMetrics,
    pub top_users: Vec<PeriodTopUser>,
    pub bot_comments: Vec<BotCommentStats>,
    pub reaction_sentiment: teloxide_statistics::sentiment::SentimentCounts,
    pub daily_active: Vec<DailyActive>,
    pub daily_activity_summary: Option<DailyActivitySummary>,
    pub retention: RetentionSummary,
    /// Live member count at report time for the engagement denominator.
    /// `None` when the Bot API lookup fails; renderers show a dash.
    pub member_count: Option<i64>,
}

#[derive(Clone)]
pub struct DailyActive {
    /// Moscow reporting date with the 05:00 boundary; zero-activity days are present.
    pub day: chrono::NaiveDate,
    pub users: i64,
}

#[derive(Clone, Copy)]
pub struct DailyActivitySummary {
    /// Mean active users across every reporting day, including zero-activity days.
    pub average_users: f64,
    pub max_users: i64,
}

impl DailyActivitySummary {
    pub fn from_days(days: &[DailyActive]) -> Option<Self> {
        if days.is_empty() {
            return None;
        }
        let total = days.iter().map(|day| i128::from(day.users)).sum::<i128>();
        let max_users = days.iter().map(|day| day.users).max().unwrap_or(0);
        Some(Self {
            average_users: total as f64 / days.len() as f64,
            max_users,
        })
    }
}

/// Cohort return rates: share of each first-day cohort active again after
/// the offset, averaged over complete cohorts. `None` without cohorts.
#[derive(Clone, Default)]
pub struct RetentionSummary {
    pub d1: Option<f64>,
    pub d7: Option<f64>,
    pub d30: Option<f64>,
}

#[derive(Clone)]
pub struct AttractionMetrics {
    /// Averages across fully completed post-comment windows only.
    pub messages_5m: String,
    pub messages_30m: String,
    pub messages_24h: String,
    pub users_30m: String,
}

#[derive(Clone)]
pub struct PeriodTopUser {
    pub user: UserPresentation,
    pub username: Option<String>,
    pub messages: i64,
    pub replies: i64,
    pub links: i64,
    /// Number of messages with supported media; voice messages are included.
    pub media: i64,
}

#[derive(Clone)]
pub struct BotCommentStats {
    pub source_message_id: i32,
    pub response: String,
    /// `None` until the complete 30-minute window fits inside the report window.
    pub messages_30m: Option<i64>,
    /// Direct human non-command replies in the same complete 30-minute window.
    pub direct_replies: Option<i64>,
    /// Latest stored count on this bot comment; zero when no snapshot row exists.
    pub reactions: i64,
}

#[derive(Clone)]
pub struct TopMessagesReportData {
    pub users: Vec<TopMessageUser>,
}

#[derive(Clone)]
pub struct TopWordReportData {
    pub word: String,
    pub users: Vec<TopWordUser>,
}

#[derive(Clone)]
pub struct TopWordUser {
    pub user: UserPresentation,
    pub username: Option<String>,
    pub occurrences: i64,
}

#[derive(Clone)]
pub struct TopMessageUser {
    pub user: UserPresentation,
    pub username: Option<String>,
    pub activity: teloxide_statistics::activity::UserActivity,
}

#[derive(Clone)]
pub struct TopReactedReportData {
    pub messages: Vec<TopReactedMessage>,
}

#[derive(Clone)]
pub struct TopReactedMessage {
    pub message_id: i32,
    pub user: UserPresentation,
    pub username: Option<String>,
    pub text: Option<String>,
    pub media: MessageMediaPreview,
    pub total_count: i64,
}

#[derive(Clone, Copy, Default)]
pub struct MessageMediaPreview {
    pub has_photo: bool,
    pub has_video: bool,
    pub has_document: bool,
    pub has_audio: bool,
    pub has_voice: bool,
    pub has_sticker: bool,
    pub has_animation: bool,
}

#[derive(Clone)]
pub struct UserStatsReportData {
    pub user: UserPresentation,
    pub username: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub(crate) profile_photo_file_id: Option<String>,
    pub(crate) profile_photo_file_unique_id: Option<String>,
    pub observed_at: Option<String>,
    pub moderation: UserModerationSummary,
    pub written_tag: Option<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub first_message_id: String,
    pub last_message_id: String,
    pub first_seen_days_ago: Option<i64>,
    pub last_seen_days_ago: Option<i64>,
    pub activity: teloxide_statistics::activity::UserActivity,
    pub activity_extras: UserActivityExtras,
    pub top_words: Vec<(String, i64)>,
}

#[derive(Clone, Default)]
pub struct UserModerationSummary {
    pub active_restriction: Option<String>,
    pub restriction_expires_at: Option<DateTime<Utc>>,
    pub active_warnings: i64,
    pub unknown_restriction: bool,
}

#[derive(Clone)]
pub struct UserActivityExtras {
    pub active_days: i64,
    /// Reaction-addition events observed for this user, not current reactions.
    pub reactions_given: i64,
    /// Human messages anywhere below a channel post in its reply tree.
    pub post_comments: i64,
    /// Human messages replying directly to a sent bot comment.
    pub replies_to_bot: i64,
}

#[derive(Clone)]
pub struct UserPresentation {
    pub user_id: i64,
    pub display_name: String,
    pub is_bot: bool,
    pub status: Option<String>,
    pub is_admin: bool,
    pub is_present: Option<bool>,
}

impl UserPresentation {
    // Keep Telegram HTML user formatting in one place so stats reports do not
    // expose raw IDs or drift into several slightly different formats.
    pub fn linked_name(&self) -> String {
        let visible = if self.display_name.trim().is_empty() {
            "пользователь"
        } else {
            self.display_name.trim()
        };

        Html::link(visible, format!("tg://user?id={}", self.user_id)).into_string()
    }

    pub fn badges(&self) -> String {
        let mut parts = Vec::new();

        if self.is_bot {
            parts.push("бот");
        }

        if self.is_admin {
            parts.push("админ");
        } else if let Some(status) = self.status.as_deref() {
            parts.push(human_member_status(status));
        } else if self.is_present == Some(true) {
            parts.push("в чате");
        } else if self.is_present == Some(false) {
            parts.push("не в чате");
        }

        if parts.is_empty() {
            "статус неизвестен".to_string()
        } else {
            parts.join(", ")
        }
    }

    pub fn linked_with_badges(&self) -> String {
        format!("{} ({})", self.linked_name(), self.badges())
    }

    pub fn linked_with_known_badges(&self) -> String {
        let badges = self.badges();
        if badges == "статус неизвестен" {
            self.linked_name()
        } else {
            format!("{} ({badges})", self.linked_name())
        }
    }
}

pub fn display_name(
    username: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
    fallback_user_id: i64,
) -> String {
    let full_name = format!(
        "{} {}",
        first_name.unwrap_or_default(),
        last_name.unwrap_or_default()
    )
    .trim()
    .to_string();

    if !full_name.is_empty() {
        return full_name;
    }

    if let Some(username) = username.filter(|value| !value.trim().is_empty()) {
        username.trim_start_matches('@').to_string()
    } else {
        fallback_user_id.to_string()
    }
}

fn human_member_status(status: &str) -> &'static str {
    match status {
        "administrator" => "админ",
        "owner" => "владелец",
        "member" => "в чате",
        "restricted" => "ограничен",
        "left" => "не в чате",
        "banned" => "забанен",
        _ => "статус неизвестен",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_prefers_real_name_over_username() {
        assert_eq!(
            display_name(Some("Chechulinm"), Some("Михаил"), Some("Чечулин"), 42),
            "Михаил Чечулин"
        );
    }

    #[test]
    fn display_name_uses_username_without_at_as_fallback() {
        assert_eq!(
            display_name(Some("@Chechulinm"), None, None, 42),
            "Chechulinm"
        );
    }
}
