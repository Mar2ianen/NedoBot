use serde_json::Value;
use sqlx::{PgPool, Row};
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId, ParseMode, ReplyParameters},
};

use crate::telegram::html;

const OWNER_USERNAME: &str = "Chechulinm";

pub struct SpamReview {
    pub id: i64,
    pub chat_id: i64,
    pub first_message_id: Option<i32>,
    pub text: String,
}

pub async fn create_high_risk_review(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
) -> anyhow::Result<Option<SpamReview>> {
    let row = sqlx::query(
        r#"
        insert into spam_review_requests (chat_id, telegram_user_id, risk_score, risk_signals)
        select a.chat_id, a.telegram_user_id, a.risk_score, a.risk_signal_breakdown
        from telegram_new_user_profile_audits a
        where a.chat_id = $1 and a.telegram_user_id = $2 and a.risk_level = 'high'
        on conflict (chat_id, telegram_user_id) do nothing
        returning id, chat_id, telegram_user_id, risk_score, risk_signals
    "#,
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let id: i64 = row.get("id");
    let score: i32 = row.get("risk_score");
    let signals: Value = row.get("risk_signals");
    let profile = sqlx::query(r#"
        select cu.first_message_id, coalesce(nullif(trim(concat_ws(' ', p.first_name, p.last_name)), ''), 'Без имени') as name,
               p.username
        from telegram_chat_users cu left join telegram_user_profiles p on p.telegram_user_id = cu.telegram_user_id
        where cu.chat_id = $1 and cu.telegram_user_id = $2
    "#).bind(chat_id).bind(user_id).fetch_one(pool).await?;
    let name: String = profile.get("name");
    let username: Option<String> = profile.get("username");
    let reasons = human_signals(&signals);
    let username = username
        .map(|value| format!("@{value}"))
        .unwrap_or_else(|| "без username".into());
    let text = format!(
        "@{OWNER_USERNAME}, <b>высокий риск спама</b>\n\n<b>{}</b> · {}\n<code>id={}</code> · риск: <b>{}</b>\n\n<b>Причины:</b>\n{}",
        html::escape(&name),
        html::escape(&username),
        user_id,
        score,
        reasons
    );
    Ok(Some(SpamReview {
        id,
        chat_id,
        first_message_id: profile.get("first_message_id"),
        text,
    }))
}

pub async fn send_review(bot: &Bot, review: &SpamReview) -> ResponseResult<()> {
    let keyboard = InlineKeyboardMarkup::new([[
        InlineKeyboardButton::callback("Верно: спамер", format!("spam_review:{}:spam", review.id)),
        InlineKeyboardButton::callback(
            "Неверно: не спамер",
            format!("spam_review:{}:normal", review.id),
        ),
    ]]);
    let mut request = bot
        .send_message(ChatId(review.chat_id), &review.text)
        .parse_mode(ParseMode::Html)
        .reply_markup(keyboard);
    if let Some(message_id) = review.first_message_id {
        request = request.reply_parameters(
            ReplyParameters::new(MessageId(message_id)).allow_sending_without_reply(),
        );
    }
    request.await?;
    Ok(())
}

pub async fn apply_callback(
    pool: &PgPool,
    request_id: i64,
    decision: &str,
    owner_id: i64,
) -> anyhow::Result<Option<&'static str>> {
    let status = match decision {
        "spam" => "confirmed_spam",
        "normal" => "confirmed_not_spam",
        _ => return Ok(None),
    };
    let mut tx = pool.begin().await?;
    let row = sqlx::query("update spam_review_requests set status = $2, reviewed_at = now(), reviewed_by_user_id = $3 where id = $1 and status = 'pending' returning chat_id, telegram_user_id")
        .bind(request_id).bind(status).bind(owner_id).fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    if decision == "spam" {
        let chat_id: i64 = row.get("chat_id");
        let user_id: i64 = row.get("telegram_user_id");
        sqlx::query("update telegram_chat_users set is_spammer = true, spam_score = greatest(spam_score, 100), spam_last_marked_at = now(), spam_reason = 'Owner-confirmed spammer', spam_type = 'llm_generic_comment', spam_types = jsonb_set(coalesce(spam_types, '{}'::jsonb), '{llm_generic_comment}', '1'::jsonb, true), updated_at = now() where chat_id = $1 and telegram_user_id = $2").bind(chat_id).bind(user_id).execute(&mut *tx).await?;
        sqlx::query("update telegram_messages set spam_marked_at = coalesce(spam_marked_at, now()), spam_reason = 'Owner-confirmed spammer', spam_source = 'manual_owner_confirmation', spam_type = coalesce(spam_type, 'llm_generic_comment') where chat_id = $1 and user_id = $2 and source_channel_id is null").bind(chat_id).bind(user_id).execute(&mut *tx).await?;
        sqlx::query("update telegram_chat_users set spam_message_count = (select count(*) from telegram_messages where chat_id = $1 and user_id = $2 and spam_marked_at is not null), spam_types = jsonb_set(coalesce(spam_types, '{}'::jsonb), '{llm_generic_comment}', to_jsonb((select count(*) from telegram_messages where chat_id = $1 and user_id = $2 and spam_marked_at is not null)), true) where chat_id = $1 and telegram_user_id = $2").bind(chat_id).bind(user_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Some(if decision == "spam" {
        "Помечено как спамер."
    } else {
        "Помечено как не спамер."
    }))
}

pub fn parse_callback(data: &str) -> Option<(i64, &str)> {
    let mut parts = data.split(':');
    (parts.next()? == "spam_review").then_some(())?;
    let id = parts.next()?.parse().ok()?;
    let decision = parts.next()?;
    parts.next().is_none().then_some((id, decision))
}

fn human_signals(signals: &Value) -> String {
    let labels = signals
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|signal| signal.get("label").and_then(Value::as_str))
        .map(human_label)
        .collect::<Vec<_>>();
    if labels.is_empty() {
        "—".to_string()
    } else {
        labels
            .into_iter()
            .map(|label| format!("• {}", html::escape(label)))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn human_label(label: &str) -> &str {
    match label {
        "recent_high_telegram_id" => "очень свежий Telegram ID",
        "single_message_account" => "первое и единственное сообщение",
        "very_new_to_chat" => "недавно появился в чате",
        "only_channel_post_comments" => "комментирует только посты канала",
        "reply_to_channel_post_not_comment" => "ответил прямо на пост, не на обсуждение",
        "display_name_reused_by_spammers" => "имя уже встречалось у размеченных спамеров",
        "username_random_suffix" => "username похож на автоматически созданный",
        "personal_channel_attached" => "подключён личный канал",
        "personal_channel_external_link" => "в личном канале есть внешняя ссылка",
        "non_adjacent_emoji_message" => "нетипичный emoji в комментарии",
        "non_adjacent_emoji_message_ending" => "комментарий заканчивается emoji",
        "first_message_spam_analysis" => "первое сообщение похоже на известную спам-кампанию",
        _ => label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_callback() {
        assert_eq!(parse_callback("spam_review:42:spam"), Some((42, "spam")));
        assert_eq!(parse_callback("spam_review:42:spam:x"), None);
    }
    #[test]
    fn renders_human_signal() {
        assert_eq!(
            human_label("recent_high_telegram_id"),
            "очень свежий Telegram ID"
        );
    }
}
