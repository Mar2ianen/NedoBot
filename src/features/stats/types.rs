use crate::telegram::html::Html;

#[derive(Clone, Copy)]
pub enum StatsPeriod {
    Day,
    Week,
    Month,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatsRender {
    Html,
    Rich,
}

#[derive(sqlx::FromRow)]
pub struct ChatStatsSummary {
    pub start_label: String,
    pub messages: i64,
    pub active_users: i64,
    pub replies: i64,
    pub links: i64,
    pub media: i64,
    pub channel_posts: i64,
    pub bot_comments: i64,
    pub replies_to_bot: i64,
    pub reaction_events: i64,
    pub reaction_count_updates: i64,
    pub bot_comment_reactions: i64,
    pub joins: i64,
    pub leaves: i64,
}

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

    fn badges(&self) -> String {
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

impl StatsPeriod {
    pub fn title(self) -> &'static str {
        match self {
            Self::Day => "день",
            Self::Week => "неделю",
            Self::Month => "месяц",
        }
    }

    pub fn start_sql(self) -> &'static str {
        // The chat day is editorial, not calendar: 05:00 Moscow time is the
        // boundary for day/week/month reports.
        match self {
            Self::Day => {
                "(date_trunc('day', now() at time zone 'Europe/Moscow' - interval '5 hours') + interval '5 hours') at time zone 'Europe/Moscow'"
            }
            Self::Week => {
                "(date_trunc('week', now() at time zone 'Europe/Moscow' - interval '5 hours') + interval '5 hours') at time zone 'Europe/Moscow'"
            }
            Self::Month => {
                "(date_trunc('month', now() at time zone 'Europe/Moscow' - interval '5 hours') + interval '5 hours') at time zone 'Europe/Moscow'"
            }
        }
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
