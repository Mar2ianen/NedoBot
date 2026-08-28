use serde_json::Value;
use teloxide::types::{
    InputRichBlock, InputRichBlockButtons, InputRichBlockExpandableBlockQuotation,
    InputRichBlockParagraph, InputRichBlockSectionHeading, InputRichMessage, RichMessageButton,
    RichText, RichTextBold, RichTextUrl,
};

use super::repo::{ReportCard, ReportResolution};
use crate::features::ask::chat_search::message_url;

pub fn render_report(card: &ReportCard) -> InputRichMessage {
    let target_name = display_name(
        card.profile_username.as_deref(),
        card.profile_first_name.as_deref(),
        card.profile_last_name.as_deref(),
        &card.target_snapshot,
        card.reported_user_id,
    );
    let reporter_name = display_name_from_snapshot(&card.reporter_snapshot, card.reporter_user_id);
    let target_url = format!("tg://user?id={}", card.reported_user_id);
    let message_link = message_url(card.chat_id, card.message_id);
    let mut blocks = vec![
        InputRichBlock::Heading(InputRichBlockSectionHeading {
            text: RichText::from("🚨 Новый репорт"),
            size: 2,
        }),
        InputRichBlock::Paragraph(paragraph([
            bold("Цель: "),
            linked(target_name, target_url.clone()),
            plain(format!(" · id={}", card.reported_user_id)),
        ])),
        InputRichBlock::Paragraph(paragraph([
            bold("Репортёр: "),
            plain(reporter_name),
            plain(format!(" · id={}", card.reporter_user_id)),
        ])),
        InputRichBlock::Paragraph(paragraph([
            bold("Сообщение: "),
            plain(format!(
                "{} · {}",
                card.target_media,
                format_time(card.target_created_at)
            )),
        ])),
    ];

    if let Some(reply_to_message_id) = card.target_reply_to_message_id {
        blocks.push(InputRichBlock::Paragraph(paragraph([
            bold("Ветка: "),
            plain(format!("reply на сообщение #{reply_to_message_id}")),
        ])));
    }

    if let Some(reason) = (!card.reason.is_empty()).then_some(card.reason.as_str()) {
        blocks.push(InputRichBlock::Paragraph(paragraph([
            bold("Причина: "),
            plain(truncate(reason, 300)),
        ])));
    }

    blocks.push(InputRichBlock::ExpandableBlockquote(
        InputRichBlockExpandableBlockQuotation {
            text: plain(truncate(
                card.target_text
                    .as_deref()
                    .unwrap_or("сообщение без текста"),
                1_500,
            )),
            credit: Some(plain("исходное сообщение")),
        },
    ));

    blocks.push(InputRichBlock::Paragraph(paragraph([
        bold("Профиль: "),
        plain(truncate(&profile_summary(card), 700)),
    ])));
    blocks.push(InputRichBlock::Paragraph(paragraph([
        bold("Активность: "),
        plain(truncate(&activity_summary(card), 350)),
    ])));
    blocks.push(InputRichBlock::Paragraph(paragraph([
        bold("Антиспам: "),
        plain(truncate(&spam_summary(card), 900)),
    ])));

    blocks.push(InputRichBlock::Buttons(buttons(
        card,
        message_link,
        target_url,
    )));
    InputRichMessage::blocks(blocks)
}

fn buttons(
    card: &ReportCard,
    message_link: Option<String>,
    target_url: String,
) -> InputRichBlockButtons {
    let mut values = Vec::new();
    if let Some(message_link) = message_link {
        values.push(RichMessageButton::url("Открыть сообщение", message_link).style("primary"));
    } else {
        values.push(RichMessageButton::disabled("Сообщение недоступно"));
    }
    values.push(RichMessageButton::url("Профиль", target_url));
    match card.resolution {
        ReportResolution::Pending => {
            values.push(
                RichMessageButton::callback("Спам", format!("report:{}:accept", card.id))
                    .style("danger"),
            );
            values.push(
                RichMessageButton::callback("Не спам", format!("report:{}:reject", card.id))
                    .style("success"),
            );
        }
        ReportResolution::Accepted => values.push(RichMessageButton::disabled("✅ Принято")),
        ReportResolution::Rejected => values.push(RichMessageButton::disabled("❌ Отклонено")),
    }
    InputRichBlockButtons::new(values).align("center")
}

fn paragraph(parts: impl IntoIterator<Item = RichText>) -> InputRichBlockParagraph {
    InputRichBlockParagraph {
        text: RichText::from(parts.into_iter().collect::<Vec<_>>()),
    }
}

fn bold(text: impl Into<String>) -> RichText {
    RichText::from(RichTextBold::new(text.into()))
}

fn plain(text: impl Into<String>) -> RichText {
    RichText::from(text.into())
}

fn linked(text: impl Into<String>, url: String) -> RichText {
    RichText::from(RichTextUrl {
        text: Box::new(plain(text)),
        url,
    })
}

fn display_name(
    username: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
    snapshot: &Value,
    user_id: i64,
) -> String {
    username
        .map(|value| format!("@{value}"))
        .or_else(|| {
            let name = [first_name, last_name]
                .into_iter()
                .flatten()
                .filter(|value| !value.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            (!name.is_empty()).then_some(name)
        })
        .or_else(|| Some(display_name_from_snapshot(snapshot, user_id)))
        .unwrap_or_else(|| format!("user {user_id}"))
}

fn display_name_from_snapshot(snapshot: &Value, user_id: i64) -> String {
    snapshot
        .get("username")
        .and_then(Value::as_str)
        .map(|value| format!("@{value}"))
        .or_else(|| {
            let name = [
                snapshot.get("first_name").and_then(Value::as_str),
                snapshot.get("last_name").and_then(Value::as_str),
            ]
            .into_iter()
            .flatten()
            .filter(|value| !value.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ");
            (!name.is_empty()).then_some(name)
        })
        .unwrap_or_else(|| format!("user {user_id}"))
}

fn profile_summary(card: &ReportCard) -> String {
    let mut values = Vec::new();
    if let Some(username) = card.profile_username.as_deref() {
        values.push(format!("@{username}"));
    }
    if card
        .profile_is_bot
        .or_else(|| card.target_snapshot.get("is_bot").and_then(Value::as_bool))
        == Some(true)
    {
        values.push("бот".to_owned());
    }
    if card.profile_is_premium == Some(true) {
        values.push("premium".to_owned());
    }
    if let Some(language) = card.profile_language_code.as_deref() {
        values.push(format!("язык {language}"));
    }
    if let Some(status) = card.member_status.as_deref() {
        values.push(format!("статус {status}"));
    }
    if card.is_admin == Some(true) {
        values.push("админ".to_owned());
    }
    if let Some(bio) = card.profile_bio.as_deref() {
        values.push(format!("bio: {}", truncate(bio, 180)));
    }
    if card.personal_channel_has_adult_links == Some(true) {
        values.push("adult-ссылки в личном канале".to_owned());
    }
    if let Some(title) = card.personal_channel_title.as_deref() {
        values.push(format!("канал: {}", truncate(title, 100)));
    }
    if let Some(username) = card.personal_channel_username.as_deref() {
        values.push(format!("канал username: @{username}"));
    }
    if let Some(is_present) = card.is_present {
        values.push(if is_present {
            "сейчас в чате".to_owned()
        } else {
            "сейчас не в чате".to_owned()
        });
    }
    if values.is_empty() {
        "данных профиля нет".to_owned()
    } else {
        values.join(" · ")
    }
}

fn activity_summary(card: &ReportCard) -> String {
    let messages = card.message_count.unwrap_or(0);
    let replies = card.reply_count.unwrap_or(0);
    let links = card.link_count.unwrap_or(0);
    let media = card.media_count.unwrap_or(0);
    format!(
        "сообщений {messages}, reply {replies}, ссылок {links}, медиа {media}; первое появление: {}; последнее: {}; статус наблюдали: {}",
        card.first_seen_at
            .map(format_time)
            .unwrap_or_else(|| "нет".to_owned()),
        card.last_seen_at
            .map(format_time)
            .unwrap_or_else(|| "нет".to_owned()),
        card.member_observed_at
            .map(format_time)
            .unwrap_or_else(|| "нет".to_owned()),
    )
}

fn spam_summary(card: &ReportCard) -> String {
    let mut values = Vec::new();
    if card.is_spammer == Some(true) {
        values.push("уже отмечен как спамер".to_owned());
    }
    if let Some(score) = card.spam_score {
        values.push(format!("score {score}"));
    }
    if let Some(kind) = card.spam_type.as_deref() {
        values.push(format!("type {kind}"));
    }
    if let Some(reason) = card.spam_reason.as_deref() {
        values.push(format!("причина {}", truncate(reason, 160)));
    }
    if let Some(types) = compact_json(card.spam_types.as_ref(), 180) {
        values.push(format!("типы {types}"));
    }
    if let Some(labels) = compact_json(card.spam_profile_labels.as_ref(), 180) {
        values.push(format!("profile labels {labels}"));
    }
    if let Some(score) = card.audit_risk_score {
        let analyzed_at = card
            .audit_analyzed_at
            .map(|value| format!(" @{}", format_time(value)))
            .unwrap_or_default();
        values.push(format!(
            "audit {score}/{}{}",
            card.audit_risk_level.as_deref().unwrap_or("?"),
            analyzed_at
        ));
    }
    if let Some(class) = card.audit_primary_risk_class.as_deref() {
        values.push(format!("class {class}"));
    }
    if let Some(labels) = compact_json(card.audit_risk_labels.as_ref(), 180) {
        values.push(format!("audit labels {labels}"));
    }
    if let Some(reasons) = compact_json(card.audit_risk_reasons.as_ref(), 220) {
        values.push(format!("audit reasons {reasons}"));
    }
    if values.is_empty() {
        "сохранённых антиспам-сигналов нет".to_owned()
    } else {
        values.join(" · ")
    }
}

fn compact_json(value: Option<&Value>, max_chars: usize) -> Option<String> {
    let value = value?;
    let rendered = match value {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .take(6)
            .map(str::to_owned)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) => serde_json::to_string(value).ok()?,
        _ => value.to_string(),
    };
    (!rendered.is_empty()).then(|| truncate(&rendered, max_chars))
}

fn format_time(value: chrono::DateTime<chrono::Utc>) -> String {
    value.format("%Y-%m-%d %H:%M UTC").to_string()
}

fn truncate(value: &str, max_chars: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    value
        .chars()
        .take(max_chars.saturating_sub(1))
        .chain(['…'])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use teloxide::ValidateWith;

    fn card() -> ReportCard {
        ReportCard {
            id: 42,
            chat_id: -100123,
            message_id: 7,
            reporter_user_id: 1,
            reported_user_id: 2,
            reason: "реклама".to_owned(),
            target_text: Some("текст <не должен> ломать rich".to_owned()),
            target_media: "текст".to_owned(),
            target_reply_to_message_id: None,
            target_created_at: Utc::now(),
            reporter_snapshot: json!({"first_name": "Reporter"}),
            target_snapshot: json!({"first_name": "Target"}),
            resolution: ReportResolution::Pending,
            profile_username: None,
            profile_first_name: Some("Target".to_owned()),
            profile_last_name: None,
            profile_is_bot: Some(false),
            profile_is_premium: None,
            profile_language_code: None,
            profile_bio: None,
            personal_channel_title: None,
            personal_channel_username: None,
            personal_channel_has_adult_links: None,
            first_seen_at: None,
            last_seen_at: None,
            message_count: Some(1),
            reply_count: Some(0),
            link_count: Some(0),
            media_count: Some(0),
            is_spammer: Some(false),
            spam_score: Some(0),
            spam_type: None,
            spam_reason: None,
            spam_types: None,
            spam_profile_labels: None,
            member_status: Some("member".to_owned()),
            is_admin: Some(false),
            is_present: Some(true),
            member_observed_at: None,
            audit_analyzed_at: None,
            audit_risk_score: None,
            audit_risk_level: None,
            audit_primary_risk_class: None,
            audit_risk_labels: None,
            audit_risk_reasons: None,
        }
    }

    #[test]
    fn renders_native_buttons_and_typed_blocks() {
        let rendered = render_report(&card());
        let blocks = rendered.blocks_ref().expect("typed rich blocks");
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, InputRichBlock::Buttons(_)))
        );
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, InputRichBlock::ExpandableBlockquote(_)))
        );
        rendered
            .validate_with(&teloxide::RichMessageContext::Send)
            .expect("report card must pass Bot API rich-message validation");
    }
}
