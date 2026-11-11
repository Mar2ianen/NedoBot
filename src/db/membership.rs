//! Подтверждённое членство отдельно от текущего left и от наличия комментариев.
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use teloxide::types::{ChatMemberKind, Message};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipContext {
    CurrentMember,
    FormerMember,
    NonMemberHistoryUnknown,
    Banned,
    #[default]
    Unknown,
}

impl MembershipContext {
    pub fn from_snapshot(status: Option<&str>, present: Option<bool>, confirmed: bool) -> Self {
        let present = match status {
            Some("banned" | "kicked") => return Self::Banned,
            Some("creator" | "owner" | "administrator" | "member") => {
                if present == Some(false) {
                    return Self::Unknown;
                }
                Some(true)
            }
            Some("left") => {
                if present == Some(true) {
                    return Self::Unknown;
                }
                Some(false)
            }
            _ => present,
        };
        match present {
            Some(true) => Self::CurrentMember,
            Some(false) if confirmed => Self::FormerMember,
            Some(false) => Self::NonMemberHistoryUnknown,
            None => Self::Unknown,
        }
    }
}

/// Положительное наблюдение не притворяется событием вступления и не меняет
/// текущее членство. MIN/MAX сохраняют доказательство при запоздавших updates.
pub async fn record_confirmed_membership(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
    observed_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"insert into telegram_chat_users
             (chat_id,telegram_user_id,membership_first_confirmed_at,membership_last_confirmed_at)
           values ($1,$2,$3,$3)
           on conflict (chat_id,telegram_user_id) do update set
             membership_first_confirmed_at=least(telegram_chat_users.membership_first_confirmed_at,$3),
             membership_last_confirmed_at=greatest(telegram_chat_users.membership_last_confirmed_at,$3)"#,
    ).bind(chat_id).bind(user_id).bind(observed_at).execute(pool).await?;
    Ok(())
}

pub async fn load_membership_context(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
    status: Option<&str>,
    present: Option<bool>,
) -> anyhow::Result<MembershipContext> {
    let confirmed: bool = sqlx::query_scalar(
        "select exists(select 1 from telegram_chat_users where chat_id=$1 and telegram_user_id=$2 and membership_first_confirmed_at is not null)",
    ).bind(chat_id).bind(user_id).fetch_one(pool).await?;
    Ok(MembershipContext::from_snapshot(status, present, confirmed))
}

pub async fn observe_member_kind(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
    kind: &ChatMemberKind,
) -> anyhow::Result<MembershipContext> {
    if kind.is_present() {
        record_confirmed_membership(pool, chat_id, user_id, Utc::now()).await?;
        return Ok(MembershipContext::CurrentMember);
    }
    let status = if matches!(kind, ChatMemberKind::Banned(_)) {
        "banned"
    } else {
        "left"
    };
    load_membership_context(pool, chat_id, user_id, Some(status), Some(false)).await
}

pub async fn observe_service_message(pool: &PgPool, message: &Message) -> anyhow::Result<()> {
    if !message.chat.is_group() && !message.chat.is_supergroup() {
        return Ok(());
    }
    if let Some(users) = message.new_chat_members() {
        for user in users {
            record_confirmed_membership(pool, message.chat.id.0, user.id.0 as i64, message.date)
                .await?;
        }
    }
    if let Some(user) = message.left_chat_member() {
        record_confirmed_membership(pool, message.chat.id.0, user.id.0 as i64, message.date)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::MembershipContext;
    use super::MembershipContext::*;
    #[test]
    fn absence_and_unknown_history_do_not_prove_never_joined() {
        assert_eq!(
            MembershipContext::from_snapshot(Some("left"), Some(false), true),
            FormerMember
        );
        assert_eq!(
            MembershipContext::from_snapshot(Some("left"), Some(false), false),
            NonMemberHistoryUnknown
        );
        assert_eq!(MembershipContext::from_snapshot(None, None, true), Unknown);
        assert_eq!(
            MembershipContext::from_snapshot(Some("banned"), Some(false), false),
            Banned
        );
        assert_eq!(
            MembershipContext::from_snapshot(Some("restricted"), Some(false), true),
            FormerMember
        );
        assert_eq!(
            MembershipContext::from_snapshot(Some("restricted"), Some(true), false),
            CurrentMember
        );
        assert_eq!(
            MembershipContext::from_snapshot(Some("member"), Some(false), true),
            Unknown
        );
    }
}
