use crate::features::stats::types::{
    ChatStatsReportData, MessageMediaPreview, TopMessagesReportData, TopReactedReportData,
    UserStatsReportData,
};
use crate::telegram::html::{Html, truncate_text};
use crate::telegram::render::escape_html;
use crate::text::normalize_ai_markers;
use chrono::{DateTime, Utc};
use teloxide::utils::time::{DateTimeFormat, DateTimeToken, TimeContext};

pub fn chat_stats(data: &ChatStatsReportData, time: &TimeContext) -> String {
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
        "<b>Статистика за {}</b>\nПериод с {}\n\nСообщения: <b>{}</b>\nАктивных пользователей: <b>{}</b>\nРеплаи: <b>{}</b>, ссылки: <b>{}</b>, медиа: <b>{}</b>\nПосты канала: <b>{}</b>, комменты бота: <b>{}</b>\nРеплаи на бота: <b>{}</b>\nРеакции events: <b>{}</b>, count updates: <b>{}</b>\nРеакции на комменты бота: <b>{}</b>\nВходы: <b>{}</b>, выходы: <b>{}</b>\n\nЗавлечение после коммента: 5м <b>{}</b>, 30м <b>{}</b>, 24ч <b>{}</b>, людей 30м <b>{}</b>",
        data.period.title(),
        period_start,
        summary.messages,
        summary.active_users,
        summary.replies,
        summary.links,
        summary.media,
        summary.channel_posts,
        summary.bot_comments,
        summary.replies_to_bot,
        summary.reaction_events,
        summary.reaction_count_updates,
        summary.bot_comment_reactions,
        summary.joins,
        summary.leaves,
        attraction.messages_5m,
        attraction.messages_30m,
        attraction.messages_24h,
        attraction.users_30m,
    );
    if !data.top_users.is_empty() {
        report.push_str("\n\n<b>Топ пользователей</b>\n");
        report.push_str(
            &data
                .top_users
                .iter()
                .map(|row| {
                    format!(
                        "{}: <b>{}</b> соо, {} реплаев, {} ссылок, {} медиа",
                        row.user.linked_with_known_badges(),
                        row.messages,
                        row.replies,
                        row.links,
                        row.media
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    if !data.bot_comments.is_empty() {
        report.push_str("\n\n<b>Комменты бота</b>\n");
        report.push_str(
            &data
                .bot_comments
                .iter()
                .map(|row| {
                    format!(
                        "#{}: {} соо за 30м, {} реплаев, {} реакций - {}",
                        row.source_message_id,
                        row.messages_30m,
                        row.direct_replies,
                        row.reactions,
                        Html::text(truncate_text(&human_comment_preview(&row.response), 110))
                            .into_string(),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    report
}

pub(crate) fn format_datetime(time: &TimeContext, value: Option<&DateTime<Utc>>) -> String {
    value
        .and_then(|value| {
            DateTimeToken::instant_in_unix(time, value.timestamp(), DateTimeFormat::DateTime).ok()
        })
        .map(|token| token.to_html())
        .unwrap_or_else(|| "нет данных".to_string())
}

pub fn top_messages(data: &TopMessagesReportData) -> String {
    let mut report = String::from("<b>Топ пишущих</b>\nЗа всё время\n");
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

pub fn top_reacted(data: &TopReactedReportData, discussion_chat_id: i64) -> String {
    let mut report = String::from("<b>Топ сообщений по реакциям</b>\nЗа всё время\n");
    if data.messages.is_empty() {
        report.push_str("\nНет данных.");
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
                &message_preview(row.text.as_deref(), row.media),
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
    time: &TimeContext,
) -> String {
    let Some(data) = data else {
        return match requested_target.map(str::trim).filter(|value| !value.is_empty()) {
            Some(_) => "Не нашёл пользователя. Используй id, username из уже виденных ботом пользователей или reply на сообщение.".to_string(),
            None => "Не понял, кого смотреть. Отправь команду обычным сообщением, ответь ей на сообщение пользователя или передай id/username.".to_string(),
        };
    };
    format!(
        "<b>Статистика пользователя</b>\n{}\nСтатус обновлён: <code>{}</code>\nПервое сообщение: {}\nПоследнее сообщение: {}\n\nСообщения: <b>{}</b>\nРеплаи: <b>{}</b>\nКомментарии: <b>{}</b>\nРеплаи на бота: <b>{}</b>\nСсылки: <b>{}</b>, медиа: <b>{}</b>, голосовые: <b>{}</b>\nАктивных дней: <b>{}</b>\nРеакций поставил: <b>{}</b>\nРеакций получил: <b>{}</b>",
        data.user.linked_with_badges(),
        format_datetime(time, data.observed_at.as_ref()),
        linked_message(
            discussion_chat_id,
            data.first_seen_at.as_ref(),
            &data.first_message_id,
            data.first_seen_days_ago,
            time,
        ),
        linked_message(
            discussion_chat_id,
            data.last_seen_at.as_ref(),
            &data.last_message_id,
            data.last_seen_days_ago,
            time,
        ),
        data.totals.messages,
        data.totals.replies,
        data.totals.post_comments,
        data.totals.replies_to_bot,
        data.totals.links,
        data.totals.media,
        data.totals.voices,
        data.totals.active_days,
        data.reactions_given,
        data.reactions_received,
    )
}

pub fn message_preview(text: Option<&str>, media: MessageMediaPreview) -> String {
    if let Some(text) = text.map(str::trim).filter(|value| !value.is_empty()) {
        return normalize_ai_markers(text);
    }
    let media = [
        (media.has_photo, "фото"),
        (media.has_video, "видео"),
        (media.has_document, "файл"),
        (media.has_audio, "аудио"),
        (media.has_voice, "голосовое"),
        (media.has_sticker, "стикер"),
        (media.has_animation, "GIF"),
    ]
    .into_iter()
    .filter_map(|(enabled, label)| enabled.then_some(label))
    .collect::<Vec<_>>();
    if media.is_empty() {
        "сообщение без текста".to_string()
    } else {
        format!("медиа: {}", media.join(", "))
    }
}

pub fn human_comment_preview(text: &str) -> String {
    normalize_ai_markers(text)
        .replace("{CHAT_LINK}", "чат")
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
    date: Option<&DateTime<Utc>>,
    message_id: &str,
    days_ago: Option<i64>,
    time: &TimeContext,
) -> String {
    let date_label = format_datetime(time, date);
    let label = days_ago.map_or_else(
        || date_label.clone(),
        |days| format!("{date_label} ({days} дн. назад)"),
    );
    match message_id.parse::<i32>() {
        Ok(message_id) => {
            let message_link =
                Html::link(format!("#{message_id}"), message_url(chat_id, message_id))
                    .into_string();
            format!("{label} ({message_link})")
        }
        Err(_) => format!("{} (#<code>{}</code>)", date_label, escape_html(message_id)),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use teloxide::utils::time::TimeContext;

    use super::{format_datetime, linked_message};

    #[test]
    fn profile_datetime_uses_configured_render_timezone() {
        let time = TimeContext::from_name("Europe/Moscow").unwrap();
        let observed_at = DateTime::parse_from_rfc3339("2026-08-01T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let rendered = format_datetime(&time, Some(&observed_at));

        assert_eq!(
            rendered,
            r#"<tg-time unix="1785578400" format="Dt">2026-08-01 13:00</tg-time>"#
        );
    }

    #[test]
    fn missing_profile_datetime_keeps_report_fallback() {
        let time = TimeContext::from_name("Europe/Moscow").unwrap();

        assert_eq!(format_datetime(&time, None), "нет данных");
    }

    #[test]
    fn linked_profile_message_keeps_time_entity_unescaped() {
        let time = TimeContext::from_name("Europe/Moscow").unwrap();
        let first_seen_at = DateTime::parse_from_rfc3339("2026-08-01T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let rendered = linked_message(-1001932061163, Some(&first_seen_at), "42", None, &time);

        assert!(rendered.contains("<tg-time unix=\"1785578400\" format=\"Dt\">"));
        assert!(!rendered.contains("&lt;tg-time"));
        assert!(rendered.contains("<a href=\"https://t.me/c/1932061163/42\">#42</a>"));
    }
}
