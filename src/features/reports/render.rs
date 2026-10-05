use serde_json::Value;
use teloxide::types::{
    InlineKeyboardButton, InlineKeyboardMarkup, InputRichBlock,
    InputRichBlockExpandableBlockQuotation, InputRichBlockParagraph, InputRichBlockSectionHeading,
    InputRichMessage, RichText, RichTextBold, RichTextUrl,
};

use super::repo::{ReportCard, ReportResolution};

pub fn render_report(card: &ReportCard) -> InputRichMessage {
    let target_name = display_name(
        card.profile_username.as_deref(),
        card.profile_first_name.as_deref(),
        card.profile_last_name.as_deref(),
        &card.target_snapshot,
        card.reported_user_id,
    );
    let reporter_name = display_name_from_snapshot(&card.reporter_snapshot, card.reporter_user_id);
    let target_link = match profile_url(card) {
        Some(url) => linked(target_name.clone(), url),
        None => linked(
            target_name.clone(),
            format!("tg://user?id={}", card.reported_user_id),
        ),
    };
    let message_link = message_url(card.chat_id, card.message_id);
    let mut blocks = vec![InputRichBlock::Heading(InputRichBlockSectionHeading {
        text: RichText::from("🚨 Новый репорт"),
        size: 2,
    })];

    if let Some(message_link) = message_link.as_deref() {
        blocks.push(InputRichBlock::Paragraph(paragraph([linked(
            "↗ Оригинал сообщения",
            message_link.to_owned(),
        )])));
    }

    blocks.extend([
        InputRichBlock::Paragraph(paragraph([
            bold("Цель: "),
            target_link,
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
    ]);

    if let Some(reply_to_message_id) = card.target_reply_to_message_id {
        blocks.push(InputRichBlock::Paragraph(paragraph([
            bold("Ветка: "),
            plain(format!("ответ на сообщение #{reply_to_message_id}")),
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

    InputRichMessage::blocks(blocks)
}

pub fn render_report_keyboard(card: &ReportCard) -> InlineKeyboardMarkup {
    let mut rows = Vec::new();
    let mut links = Vec::new();
    if let Some(message_link) = message_url(card.chat_id, card.message_id)
        && let Ok(url) = reqwest::Url::parse(&message_link)
    {
        links.push(InlineKeyboardButton::url("Открыть сообщение", url));
    }
    let target_url = format!("tg://user?id={}", card.reported_user_id);
    if let Ok(url) = reqwest::Url::parse(&target_url) {
        links.push(InlineKeyboardButton::url("Профиль", url));
    }
    if !links.is_empty() {
        rows.push(links);
    }
    match card.resolution {
        ReportResolution::Pending => {
            rows.push(vec![
                InlineKeyboardButton::callback("Спам", format!("report:{}:accept", card.id)),
                InlineKeyboardButton::callback("Не спам", format!("report:{}:reject", card.id)),
            ]);
        }
        ReportResolution::Accepted | ReportResolution::Rejected => {}
    }
    InlineKeyboardMarkup::new(rows)
}

fn message_url(chat_id: i64, message_id: i32) -> Option<String> {
    let internal_id = chat_id.to_string().strip_prefix("-100")?.to_owned();
    Some(format!("https://t.me/c/{internal_id}/{message_id}"))
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

fn profile_url(card: &ReportCard) -> Option<String> {
    let username = card
        .profile_username
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| card.target_snapshot.get("username").and_then(Value::as_str))?
        .trim();
    let valid = (5..=32).contains(&username.len())
        && username
            .bytes()
            .all(|character| character.is_ascii_alphanumeric() || character == b'_');
    valid.then(|| format!("https://t.me/{username}"))
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
        values.push("премиум".to_owned());
    }
    if let Some(language) = card.profile_language_code.as_deref() {
        values.push(format!("язык: {language}"));
    }
    if let Some(status) = card.member_status.as_deref() {
        values.push(format!("статус: {}", human_member_status(status)));
    }
    if card.is_admin == Some(true) {
        values.push("админ".to_owned());
    }
    if let Some(bio) = card.profile_bio.as_deref() {
        values.push(format!("описание: {}", truncate(bio, 180)));
    }
    if card.personal_channel_has_adult_links == Some(true) {
        values.push("ссылки 18+ в личном канале".to_owned());
    }
    if let Some(title) = card.personal_channel_title.as_deref() {
        values.push(format!("канал: {}", truncate(title, 100)));
    }
    if let Some(username) = card.personal_channel_username.as_deref() {
        values.push(format!("имя канала: @{username}"));
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
        "сообщений: {messages}, ответов: {replies}, ссылок: {links}, медиа: {media}; первое появление: {}; последнее сообщение: {}; статус наблюдался: {}",
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
        values.push(format!("оценка спама: {score}"));
    }
    if let Some(kind) = card.spam_type.as_deref() {
        values.push(format!("тип: {}", human_spam_type(kind)));
    }
    if let Some(reason) = card.spam_reason.as_deref() {
        values.push(format!("причина {}", truncate(reason, 160)));
    }
    if let Some(types) = compact_json(card.spam_types.as_ref(), 180) {
        values.push(format!("типы {types}"));
    }
    if let Some(labels) = compact_json(card.spam_profile_labels.as_ref(), 180) {
        values.push(format!("метки профиля: {labels}"));
    }
    if let Some(score) = card.audit_risk_score {
        let analyzed_at = card
            .audit_analyzed_at
            .map(|value| format!(" @{}", format_time(value)))
            .unwrap_or_default();
        values.push(format!(
            "аудит: {score}/100, риск {}{}",
            card.audit_risk_level
                .as_deref()
                .map(human_risk_level)
                .unwrap_or("неизвестен"),
            analyzed_at
        ));
    }
    if let Some(class) = card.audit_primary_risk_class.as_deref() {
        values.push(format!("класс риска: {}", human_risk_class(class)));
    }
    if let Some(labels) =
        compact_humanized_array(card.audit_risk_labels.as_ref(), 240, human_audit_label)
    {
        values.push(format!("сигналы: {labels}"));
    }
    if let Some(reasons) =
        compact_humanized_array(card.audit_risk_reasons.as_ref(), 320, human_audit_reason)
    {
        values.push(format!("основания: {reasons}"));
    }
    if values.is_empty() {
        "сохранённых антиспам-сигналов нет".to_owned()
    } else {
        values.join(" · ")
    }
}

fn compact_json(value: Option<&Value>, max_chars: usize) -> Option<String> {
    let value = value?;
    if value.is_null()
        || matches!(value, Value::Array(items) if items.is_empty())
        || matches!(value, Value::Object(items) if items.is_empty())
    {
        return None;
    }
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

fn compact_humanized_array(
    value: Option<&Value>,
    max_chars: usize,
    humanize: fn(&str) -> String,
) -> Option<String> {
    let items = value?.as_array()?;
    let rendered = items
        .iter()
        .filter_map(Value::as_str)
        .take(8)
        .map(humanize)
        .collect::<Vec<_>>()
        .join(" · ");
    (!rendered.is_empty()).then(|| truncate(&rendered, max_chars))
}

fn human_member_status(status: &str) -> &str {
    match status {
        "creator" => "создатель",
        "administrator" => "администратор",
        "member" => "участник",
        "restricted" => "ограничен",
        "left" => "вышел",
        "kicked" => "заблокирован",
        _ => status,
    }
}

fn human_spam_type(kind: &str) -> String {
    match kind {
        "adult_personal_channel_promo" => "реклама 18+ в личном канале".to_owned(),
        "foreign_invite_link_spam" => "спам через ссылку-приглашение".to_owned(),
        "llm_profile_bait" => "подозрительный профиль".to_owned(),
        "promo_dm_bait" => "приманка в личные сообщения".to_owned(),
        "link_dropper" => "распространение ссылок".to_owned(),
        "fresh_account_risk" => "риск нового аккаунта".to_owned(),
        _ => humanize_identifier(kind),
    }
}

fn human_risk_level(level: &str) -> &str {
    match level {
        "high" => "высокий",
        "medium" => "средний",
        "low" => "низкий",
        _ => level,
    }
}

fn human_risk_class(class: &str) -> String {
    match class {
        "adult_personal_channel_promo" => "реклама 18+ в личном канале".to_owned(),
        "foreign_invite_link_spam" => "спам через ссылку-приглашение".to_owned(),
        "llm_profile_bait" => "подозрительный профиль".to_owned(),
        "promo_dm_bait" => "приманка в личные сообщения".to_owned(),
        "link_dropper" => "распространение ссылок".to_owned(),
        "fresh_account_risk" => "риск нового аккаунта".to_owned(),
        _ => humanize_identifier(class),
    }
}

fn human_audit_label(label: &str) -> String {
    match label {
        "recent_high_telegram_id" => "очень свежий Telegram ID".to_owned(),
        "single_message_account" => "первое и единственное сообщение".to_owned(),
        "very_new_to_chat" => "недавно появился в чате".to_owned(),
        "only_channel_post_comments" => "комментирует только посты канала".to_owned(),
        "reply_to_channel_post_not_comment" => {
            "отвечает прямо на пост, а не на комментарий".to_owned()
        }
        "personal_channel_attached" => "подключён личный канал".to_owned(),
        "personal_channel_external_link" => "в личном канале есть внешняя ссылка".to_owned(),
        "personal_channel_adult_links" => "в личном канале есть ссылки 18+".to_owned(),
        "personal_channel_invite_link" => "в личном канале есть ссылка-приглашение".to_owned(),
        "recent_personal_channel_id" => "очень свежий ID личного канала".to_owned(),
        "short_burst_account" => "несколько сообщений за короткое время".to_owned(),
        "chat_message_has_link" => "в сообщении есть ссылка".to_owned(),
        "invite_link_from_new_user" => "новый пользователь отправил ссылку-приглашение".to_owned(),
        "foreign_invite_link_message" => "ссылка-приглашение с иностранным текстом".to_owned(),
        "username_random_suffix" => "username похож на автоматически созданный".to_owned(),
        "username_many_digits" => "в username много цифр".to_owned(),
        "missing_profile_photo" => "нет видимой аватарки".to_owned(),
        "duplicate_message_text" => "повторяются одинаковые сообщения".to_owned(),
        "duplicate_message_texture" => "повторяется структура сообщений".to_owned(),
        "similar_message_texture" => "сообщения необычно похожи".to_owned(),
        "only_replies_or_comments" => "пишет только в ответах и комментариях".to_owned(),
        "reply_to_bot_comment" => "ответил в ветке комментария бота".to_owned(),
        "very_short_bio" => "очень короткое описание профиля".to_owned(),
        "explicit_adult_promo_bio" => "описание профиля рекламирует сервис 18+".to_owned(),
        "profile_bio_subscription_invite_offer" => {
            "описание профиля рекламирует платную подписку".to_owned()
        }
        "not_present_in_chat" => "пользователь больше не в чате".to_owned(),
        "mixed_script_profile_homoglyphs" => {
            "в имени смешаны похожие латинские и кириллические буквы".to_owned()
        }
        "atypical_feminine_first_name" => "нетипичный для чата женский шаблон имени".to_owned(),
        "display_name_reused_by_spammers" => "имя встречалось у спамеров".to_owned(),
        "username_reused_by_spammers" => "username встречался у спамеров".to_owned(),
        "display_name_reused_by_mixed_labels" => {
            "имя встречалось у разных типов пользователей".to_owned()
        }
        "username_reused_by_mixed_labels" => {
            "username встречался у разных типов пользователей".to_owned()
        }
        "display_name_reused_by_confirmed_normal" => {
            "имя встречалось только у обычных пользователей".to_owned()
        }
        "username_reused_by_confirmed_normal" => {
            "username встречался только у обычных пользователей".to_owned()
        }
        "display_name_reused_by_new_accounts" => {
            "имя повторяется у других новых аккаунтов".to_owned()
        }
        "username_reused_by_new_accounts" => {
            "username повторяется у других новых аккаунтов".to_owned()
        }
        _ => humanize_identifier(label),
    }
}

fn human_audit_reason(reason: &str) -> String {
    match reason {
        "Only one observed chat message" => "в чате замечено только одно сообщение".to_owned(),
        "Few messages concentrated in a short window" => {
            "несколько сообщений отправлены за короткое время".to_owned()
        }
        "New user posted a link" => "новый пользователь отправил ссылку".to_owned(),
        "New user posted a Telegram invite link with foreign/CJK text" => {
            "новый пользователь отправил ссылку-приглашение с иностранным текстом".to_owned()
        }
        "Very new user posted a Telegram invite link" => {
            "очень новый пользователь отправил ссылку-приглашение".to_owned()
        }
        "Telegram user id is in the recent high-id range observed by the bot" => {
            "Telegram ID попадает в недавно наблюдавшийся высокий диапазон".to_owned()
        }
        "New user only comments under channel posts" => {
            "новый пользователь комментирует только посты канала".to_owned()
        }
        "New user replies directly to a channel post, not another comment" => {
            "новый пользователь отвечает прямо на пост канала, а не на комментарий".to_owned()
        }
        "User was first seen in this chat less than six hours ago" => {
            "пользователь впервые замечен в чате менее шести часов назад".to_owned()
        }
        "User was first seen in this chat less than a day ago" => {
            "пользователь впервые замечен в чате менее суток назад".to_owned()
        }
        "User has an attached personal channel" => {
            "у пользователя подключён личный канал".to_owned()
        }
        "Attached personal channel id is in a very high range" => {
            "ID личного канала попадает в очень высокий диапазон".to_owned()
        }
        "Attached personal channel contains adult/invite promo links" => {
            "в личном канале есть ссылки 18+ или приглашения".to_owned()
        }
        "Attached personal channel contains Telegram invite links" => {
            "в личном канале есть ссылки-приглашения Telegram".to_owned()
        }
        "Attached personal channel contains an external link" => {
            "в личном канале есть внешняя ссылка".to_owned()
        }
        "No visible profile photo via Bot API" => "через Bot API не видна аватарка".to_owned(),
        "Very short bio on a new account" => "у нового аккаунта очень короткое описание".to_owned(),
        "Profile name mixes Latin and Cyrillic look-alike letters" => {
            "в имени смешаны похожие латинские и кириллические буквы".to_owned()
        }
        "New user appears only in comment/reply contexts, not as normal chat participant" => {
            "новый пользователь появляется только в комментариях и ответах".to_owned()
        }
        "New user replied to a bot first-comment thread" => {
            "новый пользователь ответил в ветке первого комментария бота".to_owned()
        }
        "Replying to an existing comment is strong evidence of genuine chat participation" => {
            "ответ на существующий комментарий — сильный признак обычного участия в чате".to_owned()
        }
        _ => "дополнительный сигнал аудита".to_owned(),
    }
}

fn humanize_identifier(value: &str) -> String {
    value.replace(['_', '-'], " ")
}

fn format_time(value: chrono::DateTime<chrono::Utc>) -> String {
    value.format("%d.%m.%Y %H:%M UTC").to_string()
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
            profile_photo_file_id: None,
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
    fn renders_rich_blocks_and_moderation_keyboard() {
        let rendered = render_report(&card());
        let blocks = rendered.blocks_ref().expect("typed rich blocks");
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, InputRichBlock::ExpandableBlockquote(_)))
        );
        let keyboard = render_report_keyboard(&card());
        assert!(keyboard.inline_keyboard[1].iter().any(|button| {
            matches!(&button.kind, teloxide::types::InlineKeyboardButtonKind::CallbackData(value) if value == "report:42:accept")
        }));
        rendered
            .validate_with(&teloxide::RichMessageContext::Send)
            .expect("report card must pass Bot API rich-message validation");
    }
    #[test]
    fn puts_original_message_link_before_report_details() {
        let rendered = render_report(&card());
        let serialized = serde_json::to_string(&rendered).expect("serialize rich report");

        assert!(serialized.contains("↗ Оригинал сообщения"));
        assert!(serialized.find("Оригинал сообщения") < serialized.find("Цель:"));
        rendered
            .validate_with(&teloxide::RichMessageContext::Send)
            .expect("report card with original-message link must pass validation");
    }

    #[test]
    fn renders_audit_details_in_russian() {
        let mut card = card();
        card.audit_risk_score = Some(71);
        card.audit_risk_level = Some("high".to_owned());
        card.audit_primary_risk_class = Some("fresh_account_risk".to_owned());
        card.audit_risk_labels = Some(json!([
            "only_channel_post_comments",
            "single_message_account"
        ]));
        card.audit_risk_reasons = Some(json!([
            "Only one observed chat message",
            "New user only comments under channel posts"
        ]));

        let serialized =
            serde_json::to_string(&render_report(&card)).expect("serialize rich report");

        assert!(serialized.contains("риск высокий"));
        assert!(serialized.contains("риск нового аккаунта"));
        assert!(serialized.contains("комментирует только посты канала"));
        assert!(serialized.contains("в чате замечено только одно сообщение"));
        assert!(!serialized.contains("audit labels"));
        assert!(!serialized.contains("Only one observed chat message"));
    }

    #[test]
    fn profile_photo_does_not_break_rich_report_delivery() {
        let mut card = card();
        card.profile_photo_file_id = Some("avatar-file-id".to_owned());
        let rendered = render_report(&card);
        let value = serde_json::to_value(&rendered).expect("serialize rich report");

        assert!(
            value["blocks"]
                .as_array()
                .expect("rich report blocks")
                .iter()
                .all(|block| block["type"] != "photo")
        );
        rendered
            .validate_with(&teloxide::RichMessageContext::Send)
            .expect("report card with avatar must pass Bot API rich-message validation");
    }
}
