//! Осторожные связи для уже разобранных ссылок Telegram.
//! Username не разрешаются; членство читается только из локальных снимков.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use serde_json::{Value, json};
use sqlx::PgPool;
use teloxide::{
    prelude::{Bot, Requester},
    types::ChatId,
};
use teloxide_url::{TelegramLink, TelegramPeer, find_urls, parse_telegram_link};
use tokio::time::MissedTickBehavior;

use crate::features::url_feeds::FeedSnapshot;
use crate::{config::Config, db::membership::MembershipContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRelation {
    CurrentMember,
    FormerMember,
    BannedMember,
    AffiliatedChannel,
    KnownExternal,
    Unresolved,
}

pub struct RelationContext<'a> {
    pub chat_id: i64,
    /// ID из настроенных маршрутов первых комментариев и отдельно
    /// подтверждённых через Bot API значений `linked_chat_id`.
    pub affiliated_channels: &'a HashSet<i64>,
    /// Членство учитывается только для совпадающего явного числового ID.
    pub user_membership: Option<(u64, MembershipContext)>,
}

/// Настроенные пары чата обсуждений и канала известны без вызова Bot API.
/// Связанные чаты вызывающая сторона добавляет только после проверки API.
pub fn configured_affiliated_channels(config: &Config, chat_id: i64) -> HashSet<i64> {
    config
        .first_comment_route_for_chat(chat_id)
        .map(|route| route.source_channel_id)
        .collect()
}

const LINKED_CHAT_CACHE_MAX_AGE_SECONDS: i64 = 14 * 24 * 60 * 60;
const LINKED_CHAT_REFRESH_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);

/// Refresh only operator-configured source channels. The public username/link
/// in a message is never sent to Telegram for resolution.
pub async fn refresh_configured_linked_chats(
    bot: &Bot,
    pool: &PgPool,
    config: &Config,
) -> anyhow::Result<()> {
    let source_ids = config
        .community
        .first_comment
        .routes
        .iter()
        .map(|route| route.source_channel_id)
        .collect::<HashSet<_>>();
    for source_id in source_ids {
        match bot.get_chat(ChatId(source_id)).await {
            Ok(chat) => {
                sqlx::query(
                    r#"insert into public.telegram_linked_chat_cache
                           (source_channel_id, linked_chat_id, observed_at, checked_at)
                       values ($1, $2, now(), now())
                       on conflict (source_channel_id) do update set
                           linked_chat_id = excluded.linked_chat_id,
                           observed_at = excluded.observed_at,
                           checked_at = excluded.checked_at"#,
                )
                .bind(source_id)
                .bind(chat.linked_chat_id())
                .execute(pool)
                .await?;
            }
            Err(error) => {
                // Keep the last successful observation. Freshness expires it;
                // a transient API error cannot erase a verified association.
                sqlx::query(
                    "update public.telegram_linked_chat_cache set checked_at = now() where source_channel_id = $1",
                )
                .bind(source_id)
                .execute(pool)
                .await?;
                tracing::warn!(source_channel_id = source_id, %error, "failed to refresh Telegram linked-chat association");
            }
        }
    }
    Ok(())
}

pub fn spawn_linked_chat_refresh_worker(bot: Bot, pool: PgPool, config: Config) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(LINKED_CHAT_REFRESH_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(error) = refresh_configured_linked_chats(&bot, &pool, &config).await {
                tracing::warn!(%error, "Telegram linked-chat refresh pass failed");
            }
        }
    });
}

/// Adds only fresh Bot-API-confirmed linked IDs to the configured discussion
/// channel IDs. A missing/stale row makes the reference unresolved.
pub async fn load_affiliated_peer_ids(
    pool: &PgPool,
    config: &Config,
    chat_id: i64,
) -> anyhow::Result<HashSet<i64>> {
    let routes = config
        .first_comment_route_for_chat(chat_id)
        .collect::<Vec<_>>();
    let source_ids = routes
        .iter()
        .map(|route| route.source_channel_id)
        .collect::<Vec<_>>();
    let mut ids = source_ids.iter().copied().collect::<HashSet<_>>();
    if source_ids.is_empty() {
        return Ok(ids);
    }
    let linked_ids: Vec<i64> = sqlx::query_scalar(
        r#"select linked_chat_id
           from public.telegram_linked_chat_cache
           where source_channel_id = any($1)
             and linked_chat_id is not null
             and observed_at >= now() - make_interval(secs => $2)"#,
    )
    .bind(&source_ids)
    .bind(LINKED_CHAT_CACHE_MAX_AGE_SECONDS as f64)
    .fetch_all(pool)
    .await?;
    ids.extend(linked_ids);
    Ok(ids)
}

/// Produces counts only from recent links. Raw URLs, usernames, invite hashes,
/// and message text are not copied into the audit evidence.
pub async fn recent_link_relationship_summary(
    pool: &PgPool,
    config: &Config,
    chat_id: i64,
    telegram_user_id: i64,
    feed: Option<&FeedSnapshot>,
) -> anyhow::Result<Value> {
    const MAX_MESSAGES: i64 = 64;
    const MAX_TARGETS: usize = 128;
    let affiliated = load_affiliated_peer_ids(pool, config, chat_id).await?;
    let rows: Vec<(Option<String>, Value)> = sqlx::query_as(
        r#"select text, raw_json from telegram_messages
           where chat_id = $1 and user_id = $2 and source_channel_id is null
             and created_at >= now() - interval '24 hours'
           order by created_at desc, message_id desc limit $3"#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .bind(MAX_MESSAGES)
    .fetch_all(pool)
    .await?;

    let mut affiliated_message_links = 0u16;
    let mut external_message_links = 0u16;
    let mut unresolved_telegram_links = 0u16;
    let mut external_web_url_count = 0u16;
    let mut verified_threat_feed_hit = false;
    let mut user_target_ids = HashSet::new();
    let mut scanned_targets = 0usize;
    for (text, raw_json) in rows {
        let targets = serde_json::from_value::<teloxide::types::Message>(raw_json)
            .ok()
            .map(|message| {
                crate::telegram::entities::message_url_targets(&message)
                    .into_iter()
                    .map(|target| target.value)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                text.as_deref()
                    .map(find_urls)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|target| target.value)
                    .collect()
            });
        for target in targets {
            if scanned_targets >= MAX_TARGETS {
                break;
            }
            scanned_targets += 1;
            if let Some(parsed) = teloxide_url::parse_url(&target)
                && matches!(parsed.scheme(), "http" | "https")
                && parse_telegram_link(&target).is_none()
            {
                external_web_url_count = external_web_url_count.saturating_add(1);
                verified_threat_feed_hit |= feed
                    .and_then(|snapshot| snapshot.lookup(&target, unix_now()))
                    .is_some();
                continue;
            }
            let Some(link) = parse_telegram_link(&target) else {
                continue;
            };
            match link {
                TelegramLink::UserId(id) => {
                    if let Ok(id) = i64::try_from(id) {
                        user_target_ids.insert(id);
                    }
                }
                TelegramLink::Message {
                    peer: TelegramPeer::ChatId(id),
                    ..
                } => {
                    if id == chat_id || affiliated.contains(&id) {
                        affiliated_message_links = affiliated_message_links.saturating_add(1);
                    } else {
                        external_message_links = external_message_links.saturating_add(1);
                    }
                }
                TelegramLink::Message { .. } => {
                    unresolved_telegram_links = unresolved_telegram_links.saturating_add(1);
                }
                _ => unresolved_telegram_links = unresolved_telegram_links.saturating_add(1),
            }
        }
        if scanned_targets >= MAX_TARGETS {
            break;
        }
    }

    let user_ids = user_target_ids
        .into_iter()
        .take(MAX_TARGETS)
        .collect::<Vec<_>>();
    let user_rows: Vec<(i64, Option<String>, Option<bool>, bool)> = if user_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            r#"select telegram_user_id,
                      case when member_observed_at >= now() - make_interval(secs => $3)
                           then member_status else null end,
                      case when member_observed_at >= now() - make_interval(secs => $3)
                           then is_present else null end,
                      membership_first_confirmed_at is not null
               from telegram_chat_users
               where chat_id = $1 and telegram_user_id = any($2)"#,
        )
        .bind(chat_id)
        .bind(&user_ids)
        .bind((30_i64 * 24 * 60 * 60) as f64)
        .fetch_all(pool)
        .await?
    };
    let user_membership = user_rows
        .into_iter()
        .map(|(id, status, present, confirmed)| {
            (
                id as u64,
                MembershipContext::from_snapshot(status.as_deref(), present, confirmed),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut current_member_user_links = 0u16;
    let mut former_member_user_links = 0u16;
    let mut banned_user_links = 0u16;
    let mut unresolved_user_links = 0u16;
    for user_id in user_ids {
        match user_membership.get(&(user_id as u64)) {
            Some(MembershipContext::CurrentMember) => {
                current_member_user_links = current_member_user_links.saturating_add(1);
            }
            Some(MembershipContext::FormerMember) => {
                former_member_user_links = former_member_user_links.saturating_add(1);
            }
            Some(MembershipContext::Banned) => {
                banned_user_links = banned_user_links.saturating_add(1);
            }
            _ => unresolved_user_links = unresolved_user_links.saturating_add(1),
        }
    }

    if let Some(feed) = feed {
        let qr_hashes: Vec<String> = sqlx::query_scalar(
            r#"select evidence.target_sha256::text
               from public.telegram_message_url_evidence evidence
               join public.telegram_messages message
                 on message.chat_id = evidence.chat_id and message.message_id = evidence.message_id
               where message.chat_id = $1 and message.user_id = $2
                 and message.created_at >= now() - interval '24 hours'
                 and evidence.source = 'qr' and evidence.kind = 'url'
               order by message.created_at desc, message.message_id desc
               limit 128"#,
        )
        .bind(chat_id)
        .bind(telegram_user_id)
        .fetch_all(pool)
        .await?;
        verified_threat_feed_hit |= qr_hashes
            .iter()
            .any(|hash| feed.contains_evidence_hash(hash, unix_now()));
    }

    Ok(json!({
        "mode": "observational_only",
        "source_message_window_hours": 24,
        "message_limit": MAX_MESSAGES,
        "target_limit": MAX_TARGETS,
        "scanned_target_count": scanned_targets,
        "affiliated_telegram_message_links": affiliated_message_links,
        "external_telegram_message_links": external_message_links,
        "unresolved_telegram_links": unresolved_telegram_links,
        "external_web_url_count": external_web_url_count,
        "verified_threat_feed_hit": verified_threat_feed_hit,
        "feed_snapshot": feed.map(|snapshot| json!({
            "source": snapshot.source,
            "license": snapshot.license,
            "revision": snapshot.revision,
            "upstream_commit_unix": snapshot.loaded_unix,
            "max_age_seconds": snapshot.max_age_seconds,
            "fresh": snapshot.is_fresh(unix_now()),
            "entry_count": snapshot.entry_count(),
        })),
        "linked_user_id_targets": {
            "current_members": current_member_user_links,
            "former_members": former_member_user_links,
            "banned_members": banned_user_links,
            "unresolved": unresolved_user_links,
            "snapshot_max_age_days": 30,
        },
    }))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

pub fn relation(link: &TelegramLink, context: &RelationContext<'_>) -> LinkRelation {
    match link {
        TelegramLink::UserId(id) => {
            let Some((known_id, membership)) = context.user_membership else {
                return LinkRelation::Unresolved;
            };
            if *id != known_id {
                return LinkRelation::Unresolved;
            }
            match membership {
                MembershipContext::CurrentMember => LinkRelation::CurrentMember,
                MembershipContext::FormerMember => LinkRelation::FormerMember,
                MembershipContext::Banned => LinkRelation::BannedMember,
                MembershipContext::Unknown | MembershipContext::NonMemberHistoryUnknown => {
                    LinkRelation::Unresolved
                }
            }
        }
        TelegramLink::Message {
            peer: TelegramPeer::ChatId(id),
            ..
        } => {
            if *id == context.chat_id || context.affiliated_channels.contains(id) {
                LinkRelation::AffiliatedChannel
            } else {
                LinkRelation::KnownExternal
            }
        }
        _ => LinkRelation::Unresolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_and_invites_never_gain_guessed_identity() {
        let affiliated = HashSet::new();
        let context = RelationContext {
            chat_id: -1001,
            affiliated_channels: &affiliated,
            user_membership: None,
        };
        assert_eq!(
            relation(
                &TelegramLink::Profile {
                    username: "alice".into()
                },
                &context
            ),
            LinkRelation::Unresolved
        );
        assert_eq!(
            relation(
                &TelegramLink::Invite {
                    hash: "SYNTHETIC".into()
                },
                &context
            ),
            LinkRelation::Unresolved
        );
    }

    #[test]
    fn only_configured_numeric_channel_ids_are_affiliated() {
        let affiliated = HashSet::from([-1002]);
        let context = RelationContext {
            chat_id: -1001,
            affiliated_channels: &affiliated,
            user_membership: None,
        };
        assert_eq!(
            relation(
                &TelegramLink::Message {
                    peer: TelegramPeer::ChatId(-1002),
                    message_id: 1,
                    topic_id: None,
                },
                &context
            ),
            LinkRelation::AffiliatedChannel
        );
        assert_eq!(
            relation(
                &TelegramLink::Message {
                    peer: TelegramPeer::ChatId(-1003),
                    message_id: 1,
                    topic_id: None,
                },
                &context
            ),
            LinkRelation::KnownExternal
        );
    }

    #[test]
    fn numeric_user_relation_requires_matching_known_membership() {
        let affiliated = HashSet::new();
        let context = RelationContext {
            chat_id: -1001,
            affiliated_channels: &affiliated,
            user_membership: Some((42, MembershipContext::FormerMember)),
        };
        assert_eq!(
            relation(&TelegramLink::UserId(42), &context),
            LinkRelation::FormerMember
        );
        assert_eq!(
            relation(&TelegramLink::UserId(43), &context),
            LinkRelation::Unresolved
        );
    }
}
