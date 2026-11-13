//! Контактные entities и локальная принадлежность чату, без сетевых lookup по username.
use crate::{db::membership::MembershipContext, telegram::entities::utf16_range};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use std::collections::BTreeSet;
use teloxide::types::{MessageEntity, MessageEntityKind};

pub const VERSION: &str = "contact-membership-v1";
const MAX_CONTACTS: usize = 8;
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Username(String),
    User(i64, bool),
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContactFeatures {
    pub version: String,
    pub total: usize,
    pub known_participants: usize,
    pub known_bots: usize,
    pub last_observed_nonmembers: usize,
    pub unseen_usernames: usize,
    pub unknown_membership: usize,
    pub ambiguous_usernames: usize,
    pub invalid_entities: bool,
    pub truncated: bool,
}
impl ContactFeatures {
    pub fn external_candidate(&self) -> bool {
        self.last_observed_nonmembers > 0
            || self.unseen_usernames > 0
            || self.unknown_membership > 0
    }
}
#[derive(FromRow)]
struct Known {
    telegram_user_id: i64,
    username: Option<String>,
    is_bot: bool,
    message_count: Option<i64>,
    member_status: Option<String>,
    is_present: Option<bool>,
    membership_confirmed: bool,
    observed_at: Option<DateTime<Utc>>,
}
fn targets(text: &str, raw: &Value) -> (Vec<Target>, bool, bool) {
    let Ok(entities) = serde_json::from_value::<Vec<MessageEntity>>(raw.clone()) else {
        return (vec![], true, false);
    };
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    let mut invalid = false;
    for entity in &entities {
        if !matches!(
            entity.kind,
            MessageEntityKind::Mention | MessageEntityKind::TextMention { .. }
        ) {
            continue;
        }
        let Some(range) = utf16_range(text, entity.offset, entity.length) else {
            invalid = true;
            continue;
        };
        let quoted = entities.iter().any(|e| {
            matches!(
                e.kind,
                MessageEntityKind::Code
                    | MessageEntityKind::Pre { .. }
                    | MessageEntityKind::Blockquote
                    | MessageEntityKind::ExpandableBlockquote
            ) && utf16_range(text, e.offset, e.length)
                .is_some_and(|r| r.start < range.end && range.start < r.end)
        });
        if quoted {
            continue;
        }
        let target = match &entity.kind {
            MessageEntityKind::Mention => {
                let Some(name) = text[range].strip_prefix('@') else {
                    invalid = true;
                    continue;
                };
                if name.is_empty()
                    || name.len() > 32
                    || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    invalid = true;
                    continue;
                }
                Target::Username(name.to_ascii_lowercase())
            }
            MessageEntityKind::TextMention { user } => Target::User(user.id.0 as i64, user.is_bot),
            _ => continue,
        };
        let key = match &target {
            Target::Username(n) => format!("u:{n}"),
            Target::User(id, _) => format!("i:{id}"),
        };
        if seen.insert(key) {
            result.push(target);
        }
        if result.len() > MAX_CONTACTS {
            result.truncate(MAX_CONTACTS);
            return (result, invalid, true);
        }
    }
    (result, invalid, false)
}
fn classify(author: i64, targets: &[Target], known: &[Known]) -> ContactFeatures {
    let mut f = ContactFeatures {
        version: VERSION.into(),
        ..Default::default()
    };
    for target in targets {
        let matches: Vec<_> = known
            .iter()
            .filter(|p| match target {
                Target::Username(n) => p
                    .username
                    .as_ref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(n)),
                Target::User(id, _) => p.telegram_user_id == *id,
            })
            .collect();
        if matches.iter().any(|p| p.telegram_user_id == author)
            || matches!(target,Target::User(id,_) if *id==author)
        {
            continue;
        }
        f.total += 1;
        if matches!(target, Target::User(_, true)) {
            f.known_bots += 1;
            continue;
        }
        if matches.len() > 1 {
            f.ambiguous_usernames += 1;
            continue;
        }
        let Some(p) = matches.first() else {
            match target {
                Target::Username(_) => f.unseen_usernames += 1,
                Target::User(..) => f.unknown_membership += 1,
            };
            continue;
        };
        if p.is_bot {
            f.known_bots += 1;
            continue;
        }
        let fresh = p.observed_at.is_some_and(|t| {
            (0..=86_400).contains(&Utc::now().signed_duration_since(t).num_seconds())
        });
        let state = MembershipContext::from_snapshot(
            p.member_status.as_deref(),
            p.is_present,
            p.membership_confirmed,
        );
        if fresh
            && matches!(
                state,
                MembershipContext::FormerMember
                    | MembershipContext::NonMemberHistoryUnknown
                    | MembershipContext::Banned
            )
        {
            f.last_observed_nonmembers += 1;
        } else if matches!(state, MembershipContext::CurrentMember)
            || p.message_count.is_some_and(|n| n > 0)
        {
            f.known_participants += 1;
        } else {
            f.unknown_membership += 1;
        }
    }
    f
}
/// Только chat-scoped membership/history. Отсутствие username не доказывает non-member.
pub async fn extract(
    pool: &PgPool,
    chat: i64,
    author: i64,
    text: &str,
    entities: &Value,
) -> anyhow::Result<ContactFeatures> {
    let (targets, invalid, truncated) = targets(text, entities);
    let names: Vec<_> = targets
        .iter()
        .filter_map(|t| {
            if let Target::Username(n) = t {
                Some(n.clone())
            } else {
                None
            }
        })
        .collect();
    let ids: Vec<_> = targets
        .iter()
        .filter_map(|t| {
            if let Target::User(id, _) = t {
                Some(*id)
            } else {
                None
            }
        })
        .collect();
    let known = if targets.is_empty() {
        vec![]
    } else {
        sqlx::query_as::<_,Known>(r#"with targets as (select telegram_user_id from telegram_user_profiles where telegram_user_id=any($2) or lower(username)=any($3)
            union select unnest($2::bigint[])) select t.telegram_user_id,p.username,coalesce(p.is_bot,false) as is_bot,u.message_count,
           coalesce(s.status,u.member_status) as member_status,coalesce(s.is_present,u.is_present) as is_present,
           coalesce(u.membership_first_confirmed_at is not null,false) as membership_confirmed,
           coalesce(s.observed_at,u.member_observed_at) as observed_at
           from targets t left join telegram_user_profiles p on p.telegram_user_id=t.telegram_user_id left join telegram_chat_users u on u.telegram_user_id=t.telegram_user_id and u.chat_id=$1
           left join telegram_chat_member_snapshots s on s.telegram_user_id=t.telegram_user_id and s.chat_id=$1"#)
            .bind(chat).bind(ids).bind(names).fetch_all(pool).await?
    };
    let mut f = classify(author, &targets, &known);
    f.invalid_entities = invalid;
    f.truncated = truncated;
    Ok(f)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn entities_are_utf16_typed_bounded_and_quote_aware() {
        let text = "💵 @Nikol_bt56";
        let (t, bad, _) = targets(text, &json!([{"type":"mention","offset":3,"length":11}]));
        assert_eq!(t, vec![Target::Username("nikol_bt56".into())]);
        assert!(!bad);
        assert!(classify(1, &t, &[]).external_candidate());
        assert!(targets("mail@example.com", &json!([])).0.is_empty());
        assert!(targets(text, &json!([{"type":"mention","offset":1,"length":11}])).1);
        assert!(targets(text,&json!([{"type":"mention","offset":3,"length":11},{"type":"blockquote","offset":0,"length":14}])).0.is_empty());
        assert!(!classify(1, &[Target::User(1, false)], &[]).external_candidate());
        assert!(!classify(1, &[Target::User(2, true)], &[]).external_candidate());
    }
}
