use crate::features::stats::document::{Kv, Section};
use crate::features::stats::strings::StatsStrings;
use crate::features::stats::types::{
    ChatStatsReportData, MessageMediaPreview, TopMessagesReportData, TopReactedReportData,
    UserStatsReportData,
};
use crate::telegram::html::{Html, truncate_text};
use crate::telegram::render::escape_html;
use crate::text::normalize_ai_markers;
use teloxide::utils::time::{DateTimeFormat, DateTimeToken, TimeContext};

use teloxide_statistics::sentiment::SentimentCounts;

fn bold_num(value: i64) -> String {
    format!("<b>{value}</b>")
}

fn sentiment_value(counts: &SentimentCounts, strings: &StatsStrings) -> String {
    let mut value = format!(
        "👍 {}, 👎 {}, 🤔 {}",
        bold_num(counts.positive as i64),
        bold_num(counts.negative as i64),
        bold_num(counts.undefined as i64),
    );
    if counts.unknown > 0 {
        value.push_str(&format!(
            ", {} {}",
            strings.sentiment_unknown,
            bold_num(counts.unknown as i64)
        ));
    }
    if let Some(positivity) = counts.positivity() {
        value.push_str(&format!(
            ", {} <b>{:.0}%</b>",
            strings.sentiment_positive,
            positivity * 100.0
        ));
    }
    value
}

fn per_active_value(messages: i64, active_users: i64, strings: &StatsStrings) -> String {
    match (messages, active_users) {
        (_, 0) => strings.not_available.to_string(),
        (messages, active) => format!("<b>{:.1}</b>", messages as f64 / active as f64),
    }
}

fn reply_share_value(replies: i64, messages: i64, strings: &StatsStrings) -> String {
    match teloxide_statistics::engagement::reply_share(
        replies.max(0) as u64,
        messages.max(0) as u64,
    ) {
        Some(share) => format!("<b>{:.0}%</b>", share * 100.0),
        None => strings.not_available.to_string(),
    }
}

fn daily_active_value(
    daily: &[crate::features::stats::types::DailyActive],
    strings: &StatsStrings,
) -> String {
    if daily.is_empty() {
        return strings.not_available.to_string();
    }
    let users: Vec<i64> = daily.iter().map(|day| day.users).collect();
    let avg = users.iter().sum::<i64>() as f64 / users.len() as f64;
    let max = users.into_iter().max().unwrap_or(0);
    let mut value = format!(
        "{} <b>{:.0}</b>, {} <b>{}</b>",
        strings.avg_word, avg, strings.max_word, max
    );
    let recent: Vec<String> = daily
        .iter()
        .rev()
        .take(7)
        .rev()
        .map(|day| format!("{}:{}", day.day.format("%d.%m"), day.users))
        .collect();
    if !recent.is_empty() {
        value.push_str(&format!(" ({})", recent.join(", ")));
    }
    value
}

fn retention_value(
    retention: &crate::features::stats::types::RetentionSummary,
    strings: &StatsStrings,
) -> String {
    let part = |value: Option<f64>| match value {
        Some(rate) => format!("<b>{:.0}%</b>", rate * 100.0),
        None => strings.not_available.to_string(),
    };
    format!(
        "D+1 {}, D+7 {}, D+30 {}",
        part(retention.d1),
        part(retention.d7),
        part(retention.d30)
    )
}

fn engagement_value(
    active_users: i64,
    member_count: Option<i64>,
    strings: &StatsStrings,
) -> String {
    match member_count.filter(|&total| total > 0) {
        Some(total) => match teloxide_statistics::engagement::engagement_rate(
            active_users.max(0) as u64,
            total as u64,
        ) {
            Some(rate) => format!("<b>{:.1}%</b>", rate * 100.0),
            None => strings.not_available.to_string(),
        },
        None => strings.not_available.to_string(),
    }
}

pub fn chat_stats(
    data: &ChatStatsReportData,
    time: &TimeContext,
    strings: &StatsStrings,
) -> String {
    let summary = &data.summary;
    let attraction = &data.attraction;
    let period_start = DateTimeToken::instant_in_unix(
        time,
        summary.start_at.timestamp(),
        DateTimeFormat::DateTime,
    )
    .expect("Postgres timestamptz must fit into a Telegram timestamp")
    .to_html();
    let mut report = format!(
        "<b>{} {}</b>\n{} {}",
        strings.report_title,
        strings.period_title(data.period),
        strings.period_since,
        period_start,
    );
    let summary_section = Section {
        title: None,
        rows: vec![
            Kv {
                label: strings.messages,
                value: bold_num(summary.messages),
            },
            Kv {
                label: strings.active_users,
                value: bold_num(summary.active_users),
            },
            Kv {
                label: strings.replies,
                value: format!(
                    "{}, {}: {}, {}: {}",
                    bold_num(summary.replies),
                    strings.links,
                    bold_num(summary.links),
                    strings.media,
                    bold_num(summary.media),
                ),
            },
            Kv {
                label: strings.channel_posts,
                value: format!(
                    "{}, {}: {}",
                    bold_num(summary.channel_posts),
                    strings.bot_comments,
                    bold_num(summary.bot_comments),
                ),
            },
            Kv {
                label: strings.bot_replies,
                value: bold_num(summary.replies_to_bot),
            },
            Kv {
                label: strings.reaction_events,
                value: format!(
                    "{}, {}: {}",
                    bold_num(summary.reaction_events),
                    strings.reaction_count_updates,
                    bold_num(summary.reaction_count_updates),
                ),
            },
            Kv {
                label: strings.bot_comment_reactions,
                value: bold_num(summary.bot_comment_reactions),
            },
            Kv {
                label: strings.joins,
                value: format!(
                    "{}, {}: {}",
                    bold_num(summary.joins),
                    strings.leaves,
                    bold_num(summary.leaves),
                ),
            },
            Kv {
                label: strings.sentiment_mood,
                value: sentiment_value(&data.reaction_sentiment, strings),
            },
            Kv {
                label: strings.per_active,
                value: per_active_value(summary.messages, summary.active_users, strings),
            },
            Kv {
                label: strings.reply_share,
                value: reply_share_value(summary.replies, summary.messages, strings),
            },
            Kv {
                label: strings.active_by_day,
                value: daily_active_value(&data.daily_active, strings),
            },
            Kv {
                label: strings.returns,
                value: retention_value(&data.retention, strings),
            },
            Kv {
                label: strings.engagement,
                value: engagement_value(summary.active_users, data.member_count, strings),
            },
        ],
    };
    report.push_str(&format!("\n\n{}", summary_section.emit_html()));
    report.push_str(&format!(
        "\n\n{}: {} <b>{}</b>, {} <b>{}</b>, {} <b>{}</b>, {} <b>{}</b>",
        strings.attraction,
        strings.window_5m,
        escape_html(&attraction.messages_5m),
        strings.window_30m,
        escape_html(&attraction.messages_30m),
        strings.window_24h,
        escape_html(&attraction.messages_24h),
        strings.people_30m,
        escape_html(&attraction.users_30m),
    ));
    if !data.top_users.is_empty() {
        report.push_str(&format!("\n\n<b>{}</b>\n", strings.top_users));
        report.push_str(
            &data
                .top_users
                .iter()
                .map(|row| {
                    format!(
                        "{}: <b>{}</b> {}, {} {}, {} {}, {} {}",
                        row.user.linked_with_known_badges(),
                        row.messages,
                        strings.messages_unit_short,
                        row.replies,
                        strings.replies_unit_short,
                        row.links,
                        strings.links_unit_short,
                        row.media,
                        strings.media_unit_short,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    if !data.bot_comments.is_empty() {
        report.push_str(&format!("\n\n<b>{}</b>\n", strings.bot_comments_title));
        report.push_str(
            &data
                .bot_comments
                .iter()
                .map(|row| {
                    format!(
                        "#{id}: {m30} {u30}, {d} {ru}, {r} {rd}{preview}",
                        id = row.source_message_id,
                        m30 = row.messages_30m,
                        u30 = strings.per_30m_unit,
                        d = row.direct_replies,
                        ru = strings.replies_unit_short,
                        r = row.reactions,
                        rd = strings.reactions_dash,
                        preview = Html::text(truncate_text(
                            &human_comment_preview(&row.response, strings),
                            110
                        ))
                        .into_string(),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    report
}

pub fn top_messages(data: &TopMessagesReportData, strings: &StatsStrings) -> String {
    ranked_users(data, strings.top_writers, strings)
}

pub fn bottom_messages(data: &TopMessagesReportData, strings: &StatsStrings) -> String {
    ranked_users(data, strings.quiet_ones, strings)
}

fn ranked_users(data: &TopMessagesReportData, title: &str, strings: &StatsStrings) -> String {
    let mut report = format!("<b>{title}</b>\n{}\n", strings.all_time);
    if data.users.is_empty() {
        report.push_str("\nНет данных.");
        return report;
    }
    for (index, row) in data.users.iter().enumerate() {
        report.push_str(&format!(
            "\n{}. {}: <b>{}</b> соо, {} reply, {} медиа, {} голосовых, {} ссылок, {} реакций",
            index + 1,
            row.user.linked_with_known_badges(),
            row.messages,
            row.replies,
            row.media,
            row.voices,
            row.links,
            row.reactions_received,
        ));
    }
    report
}

pub fn top_reacted(
    data: &TopReactedReportData,
    discussion_chat_id: i64,
    strings: &StatsStrings,
) -> String {
    let mut report = format!(
        "<b>{}</b>\n{}\n",
        strings.top_reacted_messages, strings.all_time
    );
    if data.messages.is_empty() {
        report.push_str(&format!("\n{}", strings.no_data));
        return report;
    }
    for (index, row) in data.messages.iter().enumerate() {
        let author_link = Html::link(
            &row.user.display_name,
            message_url(discussion_chat_id, row.message_id),
        )
        .into_string();
        report.push_str(&format!(
            "\n{}. <b>{}</b> - {}: {}",
            index + 1,
            row.total_count,
            author_link,
            Html::text(truncate_text(
                &message_preview(row.text.as_deref(), row.media, strings),
                64
            ))
            .into_string(),
        ));
    }
    report
}

pub fn user_stats(
    data: Option<&UserStatsReportData>,
    requested_target: Option<&str>,
    discussion_chat_id: i64,
    strings: &StatsStrings,
) -> String {
    let Some(data) = data else {
        return match requested_target
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(_) => strings.user_not_found_hint.to_string(),
            None => strings.user_target_hint.to_string(),
        };
    };
    let head = Section {
        title: Some(strings.user_stats_title),
        rows: vec![
            Kv {
                label: strings.status_updated,
                value: format!(
                    "<code>{}</code>",
                    escape_html(data.observed_at.as_deref().unwrap_or(strings.no_timestamp))
                ),
            },
            Kv {
                label: strings.moderation_row,
                value: moderation_summary(&data.moderation, strings),
            },
            Kv {
                label: strings.first_message,
                value: linked_message(
                    discussion_chat_id,
                    &data.first_seen_at,
                    &data.first_message_id,
                    data.first_seen_days_ago,
                    strings,
                ),
            },
            Kv {
                label: strings.last_message,
                value: linked_message(
                    discussion_chat_id,
                    &data.last_seen_at,
                    &data.last_message_id,
                    data.last_seen_days_ago,
                    strings,
                ),
            },
        ],
    };
    let activity = Section {
        title: None,
        rows: vec![
            Kv {
                label: strings.messages,
                value: bold_num(data.totals.messages),
            },
            Kv {
                label: strings.replies,
                value: bold_num(data.totals.replies),
            },
            Kv {
                label: strings.comments,
                value: bold_num(data.totals.post_comments),
            },
            Kv {
                label: strings.bot_replies,
                value: bold_num(data.totals.replies_to_bot),
            },
            Kv {
                label: strings.links,
                value: format!(
                    "{}, {}: {}, {}: {}",
                    bold_num(data.totals.links),
                    strings.media,
                    bold_num(data.totals.media),
                    strings.voices,
                    bold_num(data.totals.voices),
                ),
            },
            Kv {
                label: strings.active_days,
                value: bold_num(data.totals.active_days),
            },
            Kv {
                label: strings.reactions_given,
                value: bold_num(data.reactions_given),
            },
            Kv {
                label: strings.reactions_received,
                value: bold_num(data.reactions_received),
            },
        ],
    };
    let mut report = format!(
        "<b>{}</b>\n{}\n",
        strings.user_stats_title,
        data.user.linked_with_badges()
    );
    report.push_str(&head.emit_html());
    report.push_str("\n\n");
    report.push_str(&activity.emit_html());
    report
}

fn moderation_summary(
    summary: &crate::features::stats::types::UserModerationSummary,
    strings: &StatsStrings,
) -> String {
    let restriction = if summary.unknown_restriction {
        strings.unknown_measure.to_string()
    } else if let Some(action) = summary.active_restriction.as_deref() {
        let label = if action == "ban" {
            strings.ban_label
        } else {
            strings.mute_label
        };
        match summary.restriction_expires_at {
            Some(expires_at) => format!(
                "{label} {} {} UTC",
                strings.until_utc,
                expires_at.format("%d.%m.%Y %H:%M")
            ),
            None => format!("{label} {}", strings.forever),
        }
    } else {
        strings.no_active_restrictions.to_string()
    };
    format!(
        "{}: {}; {}: {}/3",
        strings.moderation_line,
        escape_html(&restriction),
        strings.warnings,
        summary.active_warnings
    )
}

pub fn message_preview(
    text: Option<&str>,
    media: MessageMediaPreview,
    strings: &StatsStrings,
) -> String {
    if let Some(text) = text.map(str::trim).filter(|value| !value.is_empty()) {
        return normalize_ai_markers(text);
    }
    let media = [
        (media.has_photo, strings.media_photo),
        (media.has_video, strings.media_video),
        (media.has_document, strings.media_file),
        (media.has_audio, strings.media_audio),
        (media.has_voice, strings.media_voice),
        (media.has_sticker, strings.media_sticker),
        (media.has_animation, strings.media_gif),
    ]
    .into_iter()
    .filter_map(|(enabled, label)| enabled.then_some(label))
    .collect::<Vec<_>>();
    if media.is_empty() {
        strings.message_without_text.to_string()
    } else {
        format!("{}: {}", strings.media_prefix, media.join(", "))
    }
}

pub fn human_comment_preview(text: &str, strings: &StatsStrings) -> String {
    normalize_ai_markers(text)
        .replace("{CHAT_LINK}", strings.chat_fallback_label)
        .replace("  ", " ")
        .trim()
        .to_string()
}

pub fn message_url(chat_id: i64, message_id: i32) -> String {
    let internal_chat_id = chat_id
        .to_string()
        .strip_prefix("-100")
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| chat_id.abs().to_string());
    format!("https://t.me/c/{internal_chat_id}/{message_id}")
}

fn linked_message(
    chat_id: i64,
    date_label: &str,
    message_id: &str,
    days_ago: Option<i64>,
    strings: &StatsStrings,
) -> String {
    let label = days_ago.map_or_else(
        || date_label.to_string(),
        |days| format!("{date_label} ({days} {})", strings.days_ago),
    );
    match message_id.parse::<i32>() {
        Ok(message_id) => format!(
            "{} (#<code>{}</code>)",
            Html::link(label, message_url(chat_id, message_id)).into_string(),
            message_id
        ),
        Err(_) => format!(
            "{} (#<code>{}</code>)",
            escape_html(date_label),
            escape_html(message_id)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::stats::strings::StatsStrings;
    use crate::features::stats::types::{TopMessageUser, UserPresentation};

    fn ranked_fixture() -> TopMessagesReportData {
        TopMessagesReportData {
            users: vec![TopMessageUser {
                user: UserPresentation {
                    user_id: 7,
                    display_name: "Тихий".to_string(),
                    is_bot: false,
                    status: None,
                    is_admin: false,
                    is_present: None,
                },
                username: None,
                messages: 1,
                replies: 0,
                media: 0,
                voices: 0,
                links: 0,
                reactions_received: 0,
            }],
        }
    }

    #[test]
    fn bottom_ranking_uses_quiet_title() {
        let report = bottom_messages(&ranked_fixture(), &StatsStrings::russian());
        assert!(report.contains("Тихони чата"));
        assert!(!report.contains("Топ пишущих"));
    }

    #[test]
    fn top_ranking_keeps_loud_title() {
        assert!(top_messages(&ranked_fixture(), &StatsStrings::russian()).contains("Топ пишущих"));
    }
}
