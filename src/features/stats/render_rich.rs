use crate::features::stats::render_html::{human_comment_preview, message_preview, message_url};
use crate::features::stats::types::{
    ChatStatsReportData, TopMessagesReportData, TopReactedReportData, UserStatsReportData,
};
use crate::telegram::html::{Html, truncate_text};
use crate::telegram::render::escape_html;

pub fn chat_stats(data: &ChatStatsReportData, discussion_chat_id: i64) -> String {
    let summary = &data.summary;
    let summary_table = table_no_header(&[
        vec![
            "Период с".into(),
            escape_html(&format!("{} МСК", summary.start_label)),
        ],
        vec!["Сообщения".into(), bold_num(summary.messages)],
        vec![
            "Активные пользователи".into(),
            bold_num(summary.active_users),
        ],
        vec!["Реплаи".into(), bold_num(summary.replies)],
        vec!["Ссылки".into(), bold_num(summary.links)],
        vec!["Медиа".into(), bold_num(summary.media)],
        vec!["Посты канала".into(), bold_num(summary.channel_posts)],
        vec!["Комментарии бота".into(), bold_num(summary.bot_comments)],
        vec!["Реплаи на бота".into(), bold_num(summary.replies_to_bot)],
        vec!["Реакции events".into(), bold_num(summary.reaction_events)],
        vec![
            "Reaction count updates".into(),
            bold_num(summary.reaction_count_updates),
        ],
        vec![
            "Реакции на комменты бота".into(),
            bold_num(summary.bot_comment_reactions),
        ],
        vec![
            "Входы / выходы".into(),
            format!("{} / {}", bold_num(summary.joins), bold_num(summary.leaves)),
        ],
    ]);
    let attraction = table(
        &["Окно", "Среднее"],
        &[
            vec![
                "5 минут".into(),
                format!(
                    "<strong>{}</strong> сообщений",
                    escape_html(&data.attraction.messages_5m)
                ),
            ],
            vec![
                "30 минут".into(),
                format!(
                    "<strong>{}</strong> сообщений",
                    escape_html(&data.attraction.messages_30m)
                ),
            ],
            vec![
                "Людей за 30 минут".into(),
                format!(
                    "<strong>{}</strong>",
                    escape_html(&data.attraction.users_30m)
                ),
            ],
        ],
    );
    let top_users = if data.top_users.is_empty() {
        "<p>Нет данных.</p>".to_string()
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
                        vec!["сообщения".into(), bold_num(row.messages)],
                        vec!["reply".into(), row.replies.to_string()],
                        vec!["ссылки".into(), row.links.to_string()],
                        vec!["медиа".into(), row.media.to_string()],
                    ]),
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };
    let comments = if data.bot_comments.is_empty() {
        "<p>Нет данных.</p>".to_string()
    } else {
        table(
            &["Пост", "30м", "Reply", "Реакции", "Комментарий"],
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
                        escape_html(&truncate_text(&human_comment_preview(&row.response), 120)),
                    ]
                })
                .collect::<Vec<_>>(),
        )
    };
    format!(
        "<h1>Статистика за {}</h1><details open><summary>Сводка периода</summary>{}</details><details open><summary>Завлечение после комментария</summary>{}</details><details open><summary>Топ пользователей</summary>{}</details><details><summary>Комментарии бота</summary>{}</details><hr/><footer>Rich-версия построена из общей модели данных: таблицы, секции и кликабельные профили.</footer>",
        escape_html(data.period.title()),
        summary_table,
        attraction,
        top_users,
        comments
    )
}

pub fn top_messages(data: &TopMessagesReportData) -> String {
    if data.users.is_empty() {
        return "<h1>Топ пишущих</h1><p>Нет данных.</p>".to_string();
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
        "<h1>Топ пишущих</h1>{}<details><summary>Дополнительно</summary>{}</details><hr/><footer>Имена кликабельны, основная таблица короткая; расширенные метрики спрятаны ниже.</footer>",
        table(&["#", "Кто", "Соо", "Реакции"], &rows),
        table(&["Кто", "Reply", "Медиа / voice", "Ссылки"], &details)
    )
}

pub fn top_reacted(data: &TopReactedReportData, discussion_chat_id: i64) -> String {
    if data.messages.is_empty() {
        return "<h1>Топ сообщений по реакциям</h1><p>Нет данных.</p>".to_string();
    }
    let mut previews = Vec::new();
    let rows = data
        .messages
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let message_link =
                Html::link("сообщение", message_url(discussion_chat_id, row.message_id))
                    .into_string();
            previews.push(vec![
                (index + 1).to_string(),
                message_link.clone(),
                escape_html(&truncate_text(
                    &message_preview(row.text.as_deref(), row.media),
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
        "<h1>Топ реакций</h1>{}<details><summary>Превью сообщений</summary>{}</details><hr/><footer>Автор и сообщение кликабельны; тексты спрятаны, чтобы топ не превращался в простыню.</footer>",
        table(&["#", "Автор", "❤", "Открыть"], &rows),
        table(&["#", "Ссылка", "Текст"], &previews)
    )
}

pub fn user_stats(
    data: Option<&UserStatsReportData>,
    requested_target: Option<&str>,
    discussion_chat_id: i64,
) -> String {
    let Some(data) = data else {
        return match requested_target.map(str::trim).filter(|value| !value.is_empty()) { Some(_) => "<h1>Профиль не найден</h1><p>Не нашёл пользователя. Используй id, username из уже виденных ботом пользователей или reply на сообщение.</p>".to_string(), None => "<h1>Профиль не найден</h1><p>Не понял, кого смотреть. Передай id, username или reply на сообщение.</p>".to_string() };
    };
    let mut profile_rows = vec![vec![
        "имя".into(),
        user_link(&data.username, data.user.user_id, &data.user.display_name),
    ]];
    if let Some(tag) = data
        .written_tag
        .as_deref()
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
    {
        profile_rows.push(vec!["тег".into(), escape_html(tag)]);
    }
    let top_words = if data.top_words.is_empty() {
        "нет данных".to_string()
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
    let avatar = data
        .avatar_url
        .as_deref()
        .map(|url| format!("<img src=\"{}\"/>", escape_html(url)))
        .unwrap_or_default();
    format!(
        "<h1>{}</h1>{}<details open><summary>Основное</summary>{}</details>{}<details open><summary>Активность</summary>{}</details><details><summary>Дополнительно</summary>{}</details>",
        escape_html(&data.user.display_name),
        avatar,
        table_no_header(&profile_rows),
        bio,
        table_no_header(&[
            vec!["сообщения".into(), bold_num(data.totals.messages)],
            vec!["reply".into(), bold_num(data.totals.replies)],
            vec![
                "комментарии / боту".into(),
                format!(
                    "{} / {}",
                    bold_num(data.totals.post_comments),
                    bold_num(data.totals.replies_to_bot)
                )
            ],
            vec!["ссылки".into(), bold_num(data.totals.links)],
            vec![
                "медиа / voice".into(),
                format!(
                    "{} / {}",
                    bold_num(data.totals.media),
                    bold_num(data.totals.voices)
                )
            ],
            vec!["активных дней".into(), bold_num(data.totals.active_days)],
            vec![
                "реакции".into(),
                format!(
                    "поставил {} / получил {}",
                    bold_num(data.reactions_given),
                    bold_num(data.reactions_received)
                )
            ],
        ]),
        table_no_header(&[
            vec![
                "первое сообщение".into(),
                linked_message(
                    discussion_chat_id,
                    &data.first_seen_at,
                    &data.first_message_id,
                    data.first_seen_days_ago
                )
            ],
            vec![
                "последнее сообщение".into(),
                linked_message(
                    discussion_chat_id,
                    &data.last_seen_at,
                    &data.last_message_id,
                    data.last_seen_days_ago
                )
            ],
            vec!["частые слова".into(), top_words],
        ]),
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
