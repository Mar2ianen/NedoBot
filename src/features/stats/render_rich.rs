use crate::features::stats::document::Section;
use crate::features::stats::render_html::{human_comment_preview, message_preview, message_url};
use crate::features::stats::strings::StatsStrings;
use crate::features::stats::types::{
    ChatStatsReportData, TopMessagesReportData, TopReactedReportData, TopWordReportData,
    UserStatsReportData,
};
use crate::telegram::html::{Html, truncate_text};
use crate::telegram::render::escape_html;
use teloxide::utils::time::{DateTimeFormat, DateTimeToken, TimeContext};

use teloxide_statistics::sentiment::SentimentCounts;

fn rich_sentiment_value(counts: &SentimentCounts, strings: &StatsStrings) -> String {
    let mut value = format!(
        "👍 {} / 👎 {} / 🤔 {}",
        counts.positive, counts.negative, counts.undefined,
    );
    if counts.unknown > 0 {
        value.push_str(&format!(" / ? {}", counts.unknown));
    }
    if let Some(positivity) = counts.positivity() {
        value.push_str(&format!(
            " ({} {:.0}%)",
            strings.sentiment_positive,
            positivity * 100.0
        ));
    }
    value
}

pub fn chat_stats(
    data: &ChatStatsReportData,
    discussion_chat_id: i64,
    time: &TimeContext,
    strings: &StatsStrings,
) -> String {
    let summary = &data.summary;
    let period_start = DateTimeToken::instant_in_unix(
        time,
        summary.start_at.timestamp(),
        DateTimeFormat::DateTime,
    )
    .expect("Postgres timestamptz must fit into a Telegram timestamp")
    .to_html();
    let summary_section = Section {
        title: None,
        rows: vec![
            crate::features::stats::document::Kv {
                label: strings.period_since,
                value: period_start,
            },
            crate::features::stats::document::Kv {
                label: strings.messages,
                value: bold_num(summary.messages),
            },
            crate::features::stats::document::Kv {
                label: strings.active_users,
                value: bold_num(summary.active_users),
            },
            crate::features::stats::document::Kv {
                label: strings.replies,
                value: bold_num(summary.replies),
            },
            crate::features::stats::document::Kv {
                label: strings.links,
                value: bold_num(summary.links),
            },
            crate::features::stats::document::Kv {
                label: strings.media,
                value: bold_num(summary.media),
            },
            crate::features::stats::document::Kv {
                label: strings.channel_posts,
                value: bold_num(summary.channel_posts),
            },
            crate::features::stats::document::Kv {
                label: strings.bot_comments,
                value: bold_num(summary.bot_comments),
            },
            crate::features::stats::document::Kv {
                label: strings.bot_replies,
                value: bold_num(summary.replies_to_bot),
            },
            crate::features::stats::document::Kv {
                label: strings.sentiment_mood,
                value: rich_sentiment_value(&data.reaction_sentiment, strings),
            },
            crate::features::stats::document::Kv {
                label: strings.reaction_events,
                value: bold_num(summary.reaction_events),
            },
            crate::features::stats::document::Kv {
                label: strings.reaction_count_updates,
                value: bold_num(summary.reaction_count_updates),
            },
            crate::features::stats::document::Kv {
                label: strings.bot_comment_reactions,
                value: bold_num(summary.bot_comment_reactions),
            },
            crate::features::stats::document::Kv {
                label: strings.joins_leaves,
                value: format!("{} / {}", bold_num(summary.joins), bold_num(summary.leaves)),
            },
            crate::features::stats::document::Kv {
                label: strings.per_active,
                value: match (summary.messages, summary.active_users) {
                    (_, 0) => strings.not_available.to_string(),
                    (messages, active) => {
                        format!("{:.1}", messages as f64 / active as f64)
                    }
                },
            },
            crate::features::stats::document::Kv {
                label: strings.reply_share,
                value: match teloxide_statistics::engagement::reply_share(
                    summary.replies.max(0) as u64,
                    summary.messages.max(0) as u64,
                ) {
                    Some(share) => format!("{:.0}%", share * 100.0),
                    None => strings.not_available.to_string(),
                },
            },
            crate::features::stats::document::Kv {
                label: strings.active_by_day,
                value: if data.daily_active.is_empty() {
                    strings.not_available.to_string()
                } else {
                    let users: Vec<i64> = data.daily_active.iter().map(|day| day.users).collect();
                    format!(
                        "{} {:.0}, {} {}",
                        strings.avg_word,
                        users.iter().sum::<i64>() as f64 / users.len() as f64,
                        strings.max_word,
                        users.into_iter().max().unwrap_or(0)
                    )
                },
            },
            crate::features::stats::document::Kv {
                label: strings.returns,
                value: {
                    let part = |value: Option<f64>| match value {
                        Some(rate) => format!("{:.0}%", rate * 100.0),
                        None => strings.not_available.to_string(),
                    };
                    format!(
                        "D+1 {}, D+7 {}, D+30 {}",
                        part(data.retention.d1),
                        part(data.retention.d7),
                        part(data.retention.d30)
                    )
                },
            },
            crate::features::stats::document::Kv {
                label: strings.engagement,
                value: match data.member_count.filter(|&total| total > 0) {
                    Some(total) => {
                        match teloxide_statistics::engagement::engagement_rate(
                            summary.active_users.max(0) as u64,
                            total as u64,
                        ) {
                            Some(rate) => format!("{:.1}%", rate * 100.0),
                            None => strings.not_available.to_string(),
                        }
                    }
                    None => strings.not_available.to_string(),
                },
            },
        ],
    };
    let summary_table = table_no_header(
        &summary_section
            .emit_rich_rows()
            .into_iter()
            .map(|row| vec![row[0].clone(), row[1].clone()])
            .collect::<Vec<_>>(),
    );
    let attraction = table(
        &[strings.window_col, strings.average_col],
        &[
            vec![
                strings.minutes_5.into(),
                format!(
                    "<strong>{}</strong> {}",
                    escape_html(&data.attraction.messages_5m),
                    strings.messages_unit
                ),
            ],
            vec![
                strings.minutes_30.into(),
                format!(
                    "<strong>{}</strong> {}",
                    escape_html(&data.attraction.messages_30m),
                    strings.messages_unit
                ),
            ],
            vec![
                strings.hours_24.into(),
                format!(
                    "<strong>{}</strong> {}",
                    escape_html(&data.attraction.messages_24h),
                    strings.messages_unit
                ),
            ],
            vec![
                strings.people_30m.into(),
                format!(
                    "<strong>{}</strong>",
                    escape_html(&data.attraction.users_30m)
                ),
            ],
        ],
    );
    let no_data = format!("<p>{}</p>", strings.no_data);
    let top_users = if data.top_users.is_empty() {
        no_data.clone()
    } else {
        data.top_users
            .iter()
            .enumerate()
            .map(|(index, row)| {
                format!(
                    "<details open><summary><strong>{}.</strong> {}</summary>{}</details>",
                    index + 1,
                    user_link(&row.username, row.user.user_id, &row.user.display_name),
                    table_no_header(&[
                        vec![strings.messages.into(), bold_num(row.messages)],
                        vec![strings.row_reply.into(), row.replies.to_string()],
                        vec![strings.links.into(), row.links.to_string()],
                        vec![strings.media.into(), row.media.to_string()],
                    ]),
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };
    let comments = if data.bot_comments.is_empty() {
        no_data.clone()
    } else {
        table(
            &[
                strings.col_post,
                strings.col_window_30m,
                strings.comments_table_reply,
                strings.comments_table_reactions,
                strings.comments_table_comment,
            ],
            &data
                .bot_comments
                .iter()
                .map(|row| {
                    vec![
                        Html::link(
                            format!("#{}", row.source_message_id),
                            message_url(discussion_chat_id, row.source_message_id),
                        )
                        .into_string(),
                        bold_num(row.messages_30m),
                        row.direct_replies.to_string(),
                        row.reactions.to_string(),
                        escape_html(&truncate_text(
                            &human_comment_preview(&row.response, strings),
                            120,
                        )),
                    ]
                })
                .collect::<Vec<_>>(),
        )
    };
    format!(
        "<h1>{} {}</h1><details open><summary>{}</summary>{}</details><details open><summary>{}</summary>{}</details><details open><summary>{}</summary>{}</details><details><summary>{}</summary>{}</details><hr/><footer>{}</footer>",
        strings.report_title,
        escape_html(strings.period_title(data.period)),
        strings.details_summary,
        summary_table,
        strings.details_attraction,
        attraction,
        strings.details_top_users,
        top_users,
        strings.details_bot_comments,
        comments,
        strings.footer_tables,
    )
}

pub fn top_messages(data: &TopMessagesReportData, strings: &StatsStrings) -> String {
    ranked_users(data, strings.top_writers, strings)
}

pub fn bottom_messages(data: &TopMessagesReportData, strings: &StatsStrings) -> String {
    ranked_users(data, strings.quiet_ones, strings)
}

pub fn top_word(data: &TopWordReportData, strings: &StatsStrings) -> String {
    if data.users.is_empty() {
        return format!("<p>{}</p>", strings.no_data);
    }
    let rows = data
        .users
        .iter()
        .enumerate()
        .map(|(index, row)| {
            vec![
                (index + 1).to_string(),
                user_link(&row.username, row.user.user_id, &row.user.display_name),
                format!("{} {}", bold_num(row.occurrences), strings.word_occurrences),
            ]
        })
        .collect::<Vec<_>>();
    table_no_header(&rows)
}

fn ranked_users(data: &TopMessagesReportData, title: &str, strings: &StatsStrings) -> String {
    if data.users.is_empty() {
        return format!("<h1>{title}</h1><p>{}</p>", strings.no_data);
    }
    let mut details = Vec::new();
    let rows = data
        .users
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let link = user_link(&row.username, row.user.user_id, &row.user.display_name);
            details.push(vec![
                link.clone(),
                row.replies.to_string(),
                format!("{} / {}", row.media, row.voices),
                row.links.to_string(),
            ]);
            vec![
                (index + 1).to_string(),
                link,
                bold_num(row.messages),
                row.reactions_received.to_string(),
            ]
        })
        .collect::<Vec<_>>();
    format!(
        "<h1>{title}</h1>{}<details><summary>{}</summary>{}</details><hr/><footer>{}</footer>",
        table(
            &[
                strings.col_number,
                strings.col_who,
                strings.col_messages_short,
                strings.col_reactions
            ],
            &rows
        ),
        strings.details_extra,
        table(
            &[
                strings.col_who,
                strings.comments_table_reply,
                strings.col_media_voice,
                strings.col_links
            ],
            &details
        ),
        strings.footer_clickable,
    )
}

pub fn top_reacted(
    data: &TopReactedReportData,
    discussion_chat_id: i64,
    strings: &StatsStrings,
) -> String {
    if data.messages.is_empty() {
        return format!(
            "<h1>{}</h1><p>{}</p>",
            strings.top_reacted_messages, strings.no_data
        );
    }
    let mut previews = Vec::new();
    let rows = data
        .messages
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let message_link = Html::link(
                strings.message_link_label,
                message_url(discussion_chat_id, row.message_id),
            )
            .into_string();
            previews.push(vec![
                (index + 1).to_string(),
                message_link.clone(),
                escape_html(&truncate_text(
                    &message_preview(row.text.as_deref(), row.media, strings),
                    120,
                )),
            ]);
            vec![
                (index + 1).to_string(),
                user_or_message_link(
                    &row.username,
                    row.user.user_id,
                    &row.user.display_name,
                    discussion_chat_id,
                    row.message_id,
                ),
                bold_num(row.total_count),
                message_link,
            ]
        })
        .collect::<Vec<_>>();
    format!(
        "<h1>{}</h1>{}<details><summary>{}</summary>{}</details><hr/><footer>{}</footer>",
        strings.top_reactions_short,
        table(
            &[
                strings.col_number,
                strings.col_author,
                strings.col_heart,
                strings.col_open
            ],
            &rows
        ),
        strings.details_previews,
        table(
            &[strings.col_number, strings.col_link, strings.col_text],
            &previews
        ),
        strings.footer_compact,
    )
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
            Some(_) => format!(
                "<h1>{}</h1><p>{}</p>",
                strings.profile_not_found_title, strings.user_not_found_hint
            ),
            None => format!(
                "<h1>{}</h1><p>{}</p>",
                strings.profile_not_found_title, strings.user_target_hint_short
            ),
        };
    };
    let mut profile_rows = vec![vec![
        strings.name_row.into(),
        user_link(&data.username, data.user.user_id, &data.user.display_name),
    ]];
    profile_rows.push(vec![
        strings.moderation_row.into(),
        moderation_summary(&data.moderation, strings),
    ]);
    if let Some(tag) = data
        .written_tag
        .as_deref()
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
    {
        profile_rows.push(vec![strings.tag_row.into(), escape_html(tag)]);
    }
    let top_words = if data.top_words.is_empty() {
        strings.no_timestamp.into()
    } else {
        data.top_words
            .iter()
            .map(|(word, count)| format!("{} ({count})", escape_html(word)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let bio = data
        .bio
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            format!(
                "<section><h3>Bio</h3><p>{}</p></section>",
                escape_html(value)
            )
        })
        .unwrap_or_default();
    format!(
        "<h1>{}</h1><details open><summary>{}</summary>{}</details>{}<details open><summary>{}</summary>{}</details><details><summary>{}</summary>{}</details>",
        escape_html(&data.user.display_name),
        strings.details_main,
        table_no_header(&profile_rows),
        bio,
        strings.details_activity,
        table_no_header(&[
            vec![strings.messages.into(), bold_num(data.totals.messages)],
            vec![strings.row_reply.into(), bold_num(data.totals.replies)],
            vec![
                strings.comments_pair.into(),
                format!(
                    "{} / {}",
                    bold_num(data.totals.post_comments),
                    bold_num(data.totals.replies_to_bot)
                )
            ],
            vec![strings.links.into(), bold_num(data.totals.links)],
            vec![
                strings.media_voice_pair.into(),
                format!(
                    "{} / {}",
                    bold_num(data.totals.media),
                    bold_num(data.totals.voices)
                )
            ],
            vec![
                strings.active_days.into(),
                bold_num(data.totals.active_days)
            ],
            vec![
                strings.reactions_row.into(),
                format!(
                    "{} {} / {} {}",
                    strings.reactions_pair,
                    bold_num(data.reactions_given),
                    strings.received_verb,
                    bold_num(data.reactions_received)
                )
            ],
        ]),
        table_no_header(&[
            vec![
                strings.first_message.into(),
                linked_message(
                    discussion_chat_id,
                    &data.first_seen_at,
                    &data.first_message_id,
                    data.first_seen_days_ago,
                    strings,
                )
            ],
            vec![
                strings.last_message.into(),
                linked_message(
                    discussion_chat_id,
                    &data.last_seen_at,
                    &data.last_message_id,
                    data.last_seen_days_ago,
                    strings,
                )
            ],
            vec![strings.frequent_words.into(), top_words],
        ]),
        strings.details_extra,
    )
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
        "{}; {}: {}/3",
        escape_html(&restriction),
        strings.warnings,
        summary.active_warnings
    )
}

fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut output = String::from("<table bordered striped><tr>");
    for header in headers {
        output.push_str("<th>");
        output.push_str(&escape_html(header));
        output.push_str("</th>");
    }
    output.push_str("</tr>");
    table_rows(&mut output, rows);
    output.push_str("</table>");
    output
}
fn table_no_header(rows: &[Vec<String>]) -> String {
    let mut output = String::from("<table bordered striped>");
    table_rows(&mut output, rows);
    output.push_str("</table>");
    output
}
fn table_rows(output: &mut String, rows: &[Vec<String>]) {
    for row in rows {
        output.push_str("<tr>");
        for cell in row {
            output.push_str("<td>");
            output.push_str(cell);
            output.push_str("</td>");
        }
        output.push_str("</tr>");
    }
}
fn bold_num(value: i64) -> String {
    format!("<strong>{value}</strong>")
}
fn user_link(username: &Option<String>, user_id: i64, display_name: &str) -> String {
    let username = username
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_start_matches('@'));
    let url = username
        .map(|username| format!("https://t.me/{username}"))
        .unwrap_or_else(|| format!("tg://user?id={user_id}"));
    Html::link(display_name, url).into_string()
}
fn user_or_message_link(
    username: &Option<String>,
    user_id: i64,
    display_name: &str,
    chat_id: i64,
    message_id: i32,
) -> String {
    if username
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.trim_start_matches('@').is_empty())
    {
        user_link(username, user_id, display_name)
    } else {
        Html::link(display_name, message_url(chat_id, message_id)).into_string()
    }
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
    use crate::features::stats::types::{
        TopMessageUser, TopWordReportData, TopWordUser, UserModerationSummary, UserPresentation,
        UserStatsReportData, UserTotals,
    };

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
    fn rich_top_word_omits_headings_and_renders_usage_count() {
        let data = TopWordReportData {
            word: "<слово>".to_string(),
            users: vec![TopWordUser {
                user: UserPresentation {
                    user_id: 7,
                    display_name: "Участник".to_string(),
                    is_bot: false,
                    status: None,
                    is_admin: false,
                    is_present: None,
                },
                username: None,
                occurrences: 12,
            }],
        };

        let report = top_word(&data, &StatsStrings::russian());
        assert!(report.contains("<strong>12</strong>"));
        assert!(report.contains("вхождений"));
        assert!(!report.contains("<h1>"));
        assert!(!report.contains("<th>"));
        assert!(!report.contains("За всё время"));
    }

    #[test]
    fn rich_user_stats_omits_unattached_avatar_media() {
        let data = UserStatsReportData {
            user: UserPresentation {
                user_id: 7,
                display_name: "Артём".to_string(),
                is_bot: false,
                status: None,
                is_admin: false,
                is_present: Some(true),
            },
            username: None,
            bio: None,
            avatar_url: Some("https://example.com/avatar.png".to_string()),
            profile_photo_file_id: None,
            profile_photo_file_unique_id: None,
            observed_at: None,
            moderation: UserModerationSummary::default(),
            written_tag: None,
            first_seen_at: "сегодня".to_string(),
            last_seen_at: "сегодня".to_string(),
            first_message_id: "1".to_string(),
            last_message_id: "1".to_string(),
            first_seen_days_ago: Some(0),
            last_seen_days_ago: Some(0),
            totals: UserTotals {
                messages: 0,
                replies: 0,
                links: 0,
                media: 0,
                post_comments: 0,
                replies_to_bot: 0,
                active_days: 0,
                voices: 0,
            },
            reactions_given: 0,
            reactions_received: 0,
            top_words: Vec::new(),
        };

        let report = user_stats(Some(&data), None, -1001932061163, &StatsStrings::russian());

        assert!(report.contains("Артём"));
        assert!(!report.contains("<img"));
    }
}
