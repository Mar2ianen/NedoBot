use crate::features::stats::types::{
    ChatStatsReportData, MessageMediaPreview, TopMessagesReportData, TopReactedReportData,
    UserStatsReportData,
};
use crate::telegram::html::{Html, truncate_text};
use crate::telegram::render::escape_html;
use crate::text::normalize_ai_markers;

pub fn chat_stats(data: &ChatStatsReportData) -> String {
    let summary = &data.summary;
    let attraction = &data.attraction;
    let mut report = format!(
        "<b>Статистика за {}</b>\nПериод с <code>{}</code> МСК\n\nСообщения: <b>{}</b>\nАктивных пользователей: <b>{}</b>\nРеплаи: <b>{}</b>, ссылки: <b>{}</b>, медиа: <b>{}</b>\nПосты канала: <b>{}</b>, комменты бота: <b>{}</b>\nРеплаи на бота: <b>{}</b>\nРеакции events: <b>{}</b>, count updates: <b>{}</b>\nРеакции на комменты бота: <b>{}</b>\nВходы: <b>{}</b>, выходы: <b>{}</b>\n\nЗавлечение после коммента: 5м <b>{}</b>, 30м <b>{}</b>, людей 30м <b>{}</b>",
        data.period.title(),
        escape_html(&summary.start_label),
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
        escape_html(data.observed_at.as_deref().unwrap_or("нет данных")),
        linked_message(
            discussion_chat_id,
            &data.first_seen_at,
            &data.first_message_id,
            data.first_seen_days_ago
        ),
        linked_message(
            discussion_chat_id,
            &data.last_seen_at,
            &data.last_message_id,
            data.last_seen_days_ago
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
    date_label: &str,
    message_id: &str,
    days_ago: Option<i64>,
) -> String {
    let label = days_ago.map_or_else(
        || date_label.to_string(),
        |days| format!("{date_label} ({days} дн. назад)"),
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
