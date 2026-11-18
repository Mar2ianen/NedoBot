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
    types::{ChatFullInfo, ChatId},
};
use teloxide_url::{
    Found, Kind, Source, TelegramLink, TelegramPeer, find_urls, parse_telegram_link,
};

use crate::features::jobs::worker::{WorkerId, WorkerSupervisor, run_interval_worker};
use crate::features::url_feeds::FeedSnapshot;
use crate::{
    config::Config,
    db::{
        membership::MembershipContext,
        telegram::{TelegramPeerType, replace_current_telegram_username_aliases},
        url_evidence::QrUrlRelation,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRelation {
    CurrentMember,
    FormerMember,
    BannedMember,
    AffiliatedChannel,
    KnownExternal,
    Unresolved,
}

#[derive(Clone, Copy)]
enum UsernameLinkKind {
    Any,
    Bot,
    Chat,
}

type LinkedUserSnapshot = (
    i64,
    Option<String>,
    Option<bool>,
    bool,
    bool,
    Option<String>,
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTelegramPeer {
    pub peer_id: i64,
    pub peer_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramUsernameResolution {
    Unique(ResolvedTelegramPeer),
    Ambiguous,
    Unknown,
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
                persist_chat_username_aliases(
                    pool,
                    &chat,
                    Some(TelegramPeerType::Channel),
                    "configured_chat_refresh",
                )
                .await?;
                if let Some(linked_chat_id) = chat.linked_chat_id() {
                    match bot.get_chat(ChatId(linked_chat_id)).await {
                        Ok(linked_chat) => {
                            persist_chat_username_aliases(
                                pool,
                                &linked_chat,
                                Some(TelegramPeerType::Supergroup),
                                "configured_chat_refresh",
                            )
                            .await?;
                        }
                        Err(error) => tracing::warn!(
                            linked_chat_id,
                            %error,
                            "failed to refresh configured Telegram linked-chat aliases"
                        ),
                    }
                }
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
    refresh_known_bot_aliases(bot, pool).await?;
    Ok(())
}

async fn persist_chat_username_aliases(
    pool: &PgPool,
    chat: &ChatFullInfo,
    peer_type_override: Option<TelegramPeerType>,
    source: &str,
) -> anyhow::Result<()> {
    let peer_type = peer_type_override.unwrap_or_else(|| {
        if chat.is_private() {
            TelegramPeerType::PrivateChat
        } else if chat.is_group() {
            TelegramPeerType::Group
        } else if chat.is_supergroup() {
            TelegramPeerType::Supergroup
        } else {
            TelegramPeerType::Channel
        }
    });
    let usernames = chat
        .active_usernames()
        .unwrap_or_default()
        .iter()
        .cloned()
        .chain(chat.username().map(str::to_owned))
        .collect::<Vec<_>>();
    replace_current_telegram_username_aliases(pool, chat.id.0, peer_type, source, &usernames).await
}

async fn refresh_known_bot_aliases(bot: &Bot, pool: &PgPool) -> anyhow::Result<()> {
    const MAX_BOTS_PER_PASS: i64 = 32;
    let bot_ids: Vec<i64> = sqlx::query_scalar(
        r#"select profile.telegram_user_id
             from public.telegram_user_profiles profile
            where profile.is_bot and profile.username is not null
              and not exists (
                    select 1 from public.telegram_username_aliases alias
                     where alias.telegram_peer_id = profile.telegram_user_id
                       and alias.peer_type = 'bot'
                       and alias.source = 'known_bot_refresh'
                       and alias.is_active
                       and alias.observed_at >= now() - interval '12 hours'
              )
            order by profile.updated_at asc
            limit $1"#,
    )
    .bind(MAX_BOTS_PER_PASS)
    .fetch_all(pool)
    .await?;

    for bot_id in bot_ids {
        match bot.get_chat(ChatId(bot_id)).await {
            Ok(chat) => {
                if let Err(error) = persist_chat_username_aliases(
                    pool,
                    &chat,
                    Some(TelegramPeerType::Bot),
                    "known_bot_refresh",
                )
                .await
                {
                    tracing::warn!(bot_id, %error, "failed to store known bot username aliases");
                }
            }
            Err(error) => {
                tracing::debug!(bot_id, %error, "failed to refresh known bot username aliases")
            }
        }
    }
    Ok(())
}

pub fn spawn_linked_chat_refresh_worker(
    supervisor: &mut WorkerSupervisor,
    bot: Bot,
    pool: PgPool,
    config: Config,
) {
    supervisor.spawn(WorkerId::LinkedChatRefresh, move |shutdown| async move {
        run_interval_worker(
            WorkerId::LinkedChatRefresh,
            shutdown,
            LINKED_CHAT_REFRESH_INTERVAL,
            true,
            true,
            || async {
                refresh_configured_linked_chats(&bot, &pool, &config)
                    .await
                    .err()
                    .map(|_| "linked_chat_refresh_failed")
            },
        )
        .await;
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
    let mut verified_threat_feed_hit = false;
    let mut user_target_ids = HashSet::new();
    let mut username_targets = HashSet::new();
    let mut pending_username_links = Vec::<(String, UsernameLinkKind)>::new();
    let mut bot_candidate_usernames = HashSet::new();
    let mut scanned_targets = 0usize;
    let mut native_user_id_entity_count = 0u16;
    let mut uri_user_id_count = 0u16;
    for (text, raw_json) in rows {
        let targets = relation_targets(raw_json, text.as_deref());
        for target in targets {
            if scanned_targets >= MAX_TARGETS {
                break;
            }
            scanned_targets += 1;
            if let Kind::UserId(id) = target.kind {
                if let Ok(id) = i64::try_from(id) {
                    user_target_ids.insert(id);
                    if is_native_user_mention(target.source) {
                        native_user_id_entity_count = native_user_id_entity_count.saturating_add(1);
                    } else {
                        uri_user_id_count = uri_user_id_count.saturating_add(1);
                    }
                }
                continue;
            }
            if target.kind == Kind::Mention {
                if let Some(username) = normalize_username(&target.value) {
                    if is_bot_candidate(&username) {
                        bot_candidate_usernames.insert(username.clone());
                    }
                    username_targets.insert(username);
                }
                continue;
            }
            if target.kind != Kind::Url {
                continue;
            }
            let target = target.value;
            if let Some(parsed) = teloxide_url::parse_url(&target)
                && matches!(parsed.scheme(), "http" | "https")
                && parse_telegram_link(&target).is_none()
            {
                verified_threat_feed_hit |= feed
                    .and_then(|snapshot| snapshot.lookup(&target, unix_now()))
                    .is_some();
                continue;
            }
            let Some(link) = parse_telegram_link(&target) else {
                continue;
            };
            match &link {
                TelegramLink::Profile { username } => {
                    let username = username.to_ascii_lowercase();
                    if is_bot_candidate(&username) {
                        bot_candidate_usernames.insert(username.clone());
                    }
                    username_targets.insert(username.clone());
                    pending_username_links.push((username, UsernameLinkKind::Any));
                }
                TelegramLink::BotStart { username, .. } => {
                    let username = username.to_ascii_lowercase();
                    if is_bot_candidate(&username) {
                        bot_candidate_usernames.insert(username.clone());
                    }
                    username_targets.insert(username.clone());
                    pending_username_links.push((username, UsernameLinkKind::Bot));
                }
                TelegramLink::Message {
                    peer: TelegramPeer::Username(username),
                    ..
                } => {
                    let username = username.to_ascii_lowercase();
                    if is_bot_candidate(&username) {
                        bot_candidate_usernames.insert(username.clone());
                    }
                    username_targets.insert(username.clone());
                    pending_username_links.push((username, UsernameLinkKind::Chat));
                }
                _ => {}
            }
            match link {
                TelegramLink::UserId(id) => {
                    if let Ok(id) = i64::try_from(id) {
                        user_target_ids.insert(id);
                        uri_user_id_count = uri_user_id_count.saturating_add(1);
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
                TelegramLink::Message {
                    peer: TelegramPeer::Username(_),
                    ..
                } => {}
                _ => unresolved_telegram_links = unresolved_telegram_links.saturating_add(1),
            }
        }
        if scanned_targets >= MAX_TARGETS {
            break;
        }
    }

    // Resolve only fresh aliases from authoritative current snapshots. History is
    // deliberately excluded because Telegram may reassign usernames.
    let usernames = username_targets.into_iter().collect::<Vec<_>>();
    let resolved_names = resolve_current_username_aliases(pool, &usernames).await?;
    let mut resolved_username_count = 0u16;
    let mut confirmed_bot_candidate_count = 0u16;
    let mut resolved_bot_count = 0u16;
    let mut resolved_chat_count = 0u16;
    let mut resolved_user_count = 0u16;
    for (username, resolution) in &resolved_names {
        let TelegramUsernameResolution::Unique(peer) = resolution else {
            continue;
        };
        let peer_id = peer.peer_id;
        let peer_type = &peer.peer_type;
        if matches!(peer_type.as_str(), "user" | "bot") {
            user_target_ids.insert(peer_id);
        }
        resolved_username_count = resolved_username_count.saturating_add(1);
        match peer_type.as_str() {
            "bot" => {
                resolved_bot_count = resolved_bot_count.saturating_add(1);
                if bot_candidate_usernames.contains(username) {
                    confirmed_bot_candidate_count = confirmed_bot_candidate_count.saturating_add(1);
                }
            }
            "user" => resolved_user_count = resolved_user_count.saturating_add(1),
            "private_chat" | "group" | "supergroup" | "channel" => {
                resolved_chat_count = resolved_chat_count.saturating_add(1)
            }
            _ => {}
        }
    }
    for (username, kind) in pending_username_links {
        let resolved = match resolved_names.get(&username) {
            Some(TelegramUsernameResolution::Unique(peer)) => Some(peer),
            Some(TelegramUsernameResolution::Ambiguous | TelegramUsernameResolution::Unknown)
            | None => None,
        };
        match (kind, resolved) {
            (UsernameLinkKind::Any, Some(_)) => {}
            (UsernameLinkKind::Bot, Some(peer)) if peer.peer_type == "bot" => {}
            (UsernameLinkKind::Chat, Some(peer))
                if matches!(peer.peer_type.as_str(), "supergroup" | "channel" | "group") =>
            {
                if peer.peer_id == chat_id || affiliated.contains(&peer.peer_id) {
                    affiliated_message_links = affiliated_message_links.saturating_add(1);
                } else {
                    external_message_links = external_message_links.saturating_add(1);
                }
            }
            _ => unresolved_telegram_links = unresolved_telegram_links.saturating_add(1),
        }
    }

    let user_ids = user_target_ids
        .into_iter()
        .take(MAX_TARGETS)
        .collect::<Vec<_>>();
    let user_rows: Vec<LinkedUserSnapshot> = if user_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            r#"select user_row.telegram_user_id,
                      case when member_observed_at >= now() - make_interval(secs => $3)
                           then member_status else null end,
                      case when member_observed_at >= now() - make_interval(secs => $3)
                           then is_present else null end,
                      membership_first_confirmed_at is not null,
                      coalesce(user_row.is_spammer, false), human_label.label
               from telegram_chat_users user_row
               left join lateral (
                    select event.label
                      from public.spam_label_events event
                     where event.chat_id = user_row.chat_id
                       and event.telegram_user_id = user_row.telegram_user_id
                       and event.source in ('owner_review', 'owner_manual', 'legacy_backfill')
                     order by event.created_at desc, event.id desc
                     limit 1
               ) human_label on true
               where user_row.chat_id = $1 and user_row.telegram_user_id = any($2)"#,
        )
        .bind(chat_id)
        .bind(&user_ids)
        .bind((30_i64 * 24 * 60 * 60) as f64)
        .fetch_all(pool)
        .await?
    };
    let mut currently_flagged_spammer_user_links = 0u16;
    let mut operator_confirmed_spammer_user_links = 0u16;
    let mut operator_confirmed_normal_user_links = 0u16;
    let user_membership = user_rows
        .into_iter()
        .map(
            |(id, status, present, confirmed, is_spammer, human_label)| {
                if is_spammer {
                    currently_flagged_spammer_user_links =
                        currently_flagged_spammer_user_links.saturating_add(1);
                }
                match human_label.as_deref() {
                    Some("spam") => {
                        operator_confirmed_spammer_user_links =
                            operator_confirmed_spammer_user_links.saturating_add(1);
                    }
                    Some("not_spam") => {
                        operator_confirmed_normal_user_links =
                            operator_confirmed_normal_user_links.saturating_add(1);
                    }
                    _ => {}
                }
                (
                    id as u64,
                    MembershipContext::from_snapshot(status.as_deref(), present, confirmed),
                )
            },
        )
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

    let url_target_rows: Vec<(String, Option<String>, String, bool)> = sqlx::query_as(
        r#"select evidence.target_sha256::text,
                  min(evidence.host),
                  min(evidence.display_hint),
                  bool_or(evidence.source = 'qr' and evidence.qr_relation = 'external')
           from public.telegram_message_url_evidence evidence
           join public.telegram_messages message
             on message.chat_id = evidence.chat_id and message.message_id = evidence.message_id
           where message.chat_id = $1 and message.user_id = $2
             and message.created_at >= now() - interval '24 hours'
             and evidence.parser_version = $3 and evidence.kind = 'url'
           group by evidence.target_sha256
           order by max(message.created_at) desc
           limit 128"#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .bind(crate::db::url_evidence::URL_EVIDENCE_PARSER_VERSION)
    .fetch_all(pool)
    .await?;
    let unique_external_url_count = url_target_rows
        .iter()
        .filter(|(_, host, display_hint, _)| {
            (display_hint.starts_with("http://") || display_hint.starts_with("https://"))
                && host.as_deref().is_some_and(is_external_web_host)
        })
        .count()
        .min(u16::MAX as usize) as u16;
    let external_qr_url_count = url_target_rows
        .iter()
        .filter(|(_, _, _, has_external_qr)| *has_external_qr)
        .count()
        .min(u16::MAX as usize) as u16;

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
        "locally_resolved_current_usernames": resolved_username_count,
        "locally_resolved_username_count": resolved_username_count,
        "locally_resolved_users": resolved_user_count,
        "locally_resolved_bots": resolved_bot_count,
        "locally_resolved_chats": resolved_chat_count,
        "confirmed_bot_candidates": confirmed_bot_candidate_count,
        "currently_flagged_spammer_user_links": currently_flagged_spammer_user_links,
        "operator_confirmed_spammer_user_links": operator_confirmed_spammer_user_links,
        "operator_confirmed_normal_user_links": operator_confirmed_normal_user_links,
        "username_snapshot_max_age_hours": 24,
        "unique_external_url_count": unique_external_url_count,
        // Compatibility/diagnostic alias; URL shadow uses only the unified unique count.
        "external_web_url_count": unique_external_url_count,
        "external_qr_url_count": external_qr_url_count,
        "verified_threat_feed_hit": verified_threat_feed_hit,
        "user_id_target_provenance": {
            "telegram_text_mention_entities": native_user_id_entity_count,
            "telegram_user_uri_links": uri_user_id_count,
        },
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

async fn resolve_current_username_aliases(
    pool: &PgPool,
    usernames: &[String],
) -> anyhow::Result<HashMap<String, TelegramUsernameResolution>> {
    if usernames.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(String, i64, String, i64)> = sqlx::query_as(
        r#"select username_normalized, min(telegram_peer_id), min(peer_type),
                  count(distinct (telegram_peer_id, peer_type))::bigint
             from public.telegram_username_aliases
            where username_normalized = any($1)
              and is_active
              and observed_at >= now() - interval '24 hours'
            group by username_normalized"#,
    )
    .bind(usernames)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(username, peer_id, peer_type, owner_count)| {
            let resolution = if owner_count == 1 {
                TelegramUsernameResolution::Unique(ResolvedTelegramPeer { peer_id, peer_type })
            } else {
                TelegramUsernameResolution::Ambiguous
            };
            (username, resolution)
        })
        .collect())
}

/// Resolves a Telegram username only from a fresh, unique local snapshot.
/// Unknown and conflicting aliases remain distinct fail-closed outcomes.
pub async fn resolve_telegram_username(
    pool: &PgPool,
    username: &str,
) -> anyhow::Result<TelegramUsernameResolution> {
    let Some(username) = normalize_username(username) else {
        return Ok(TelegramUsernameResolution::Unknown);
    };
    let resolutions =
        resolve_current_username_aliases(pool, std::slice::from_ref(&username)).await?;
    Ok(resolutions
        .get(&username)
        .cloned()
        .unwrap_or(TelegramUsernameResolution::Unknown))
}

fn is_external_web_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    ![
        "t.me",
        "telegram.me",
        "telegram.dog",
        "www.t.me",
        "www.telegram.me",
        "www.telegram.dog",
    ]
    .contains(&host.as_str())
        && !host.ends_with(".t.me")
        && !host.ends_with(".telegram.me")
        && !host.ends_with(".telegram.dog")
}

fn normalize_username(value: &str) -> Option<String> {
    let username = value.trim().trim_start_matches('@').to_ascii_lowercase();
    (5..=32).contains(&username.len()).then_some(())?;
    username
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        .then_some(username)
}

fn is_bot_candidate(username: &str) -> bool {
    ["bot", "agent", "ai", "app", "game", "gay"]
        .iter()
        .any(|suffix| username.ends_with(suffix))
}

fn relation_targets(raw_json: Value, text: Option<&str>) -> Vec<Found> {
    serde_json::from_value::<teloxide::types::Message>(raw_json)
        .ok()
        .map(|message| teloxide_url::adapter::extract_from_message(&message))
        .unwrap_or_else(|| text.map(find_urls).unwrap_or_default())
}

fn is_native_user_mention(source: Source) -> bool {
    matches!(source, Source::TelegramEntity | Source::RichMessage)
}

pub fn classify_qr_url(
    target: &str,
    chat_id: i64,
    affiliated_channels: &HashSet<i64>,
) -> QrUrlRelation {
    domain::classify_qr_url(target, chat_id, affiliated_channels)
}

pub async fn classify_qr_url_with_aliases(
    pool: &PgPool,
    target: &str,
    chat_id: i64,
    affiliated_channels: &HashSet<i64>,
) -> anyhow::Result<QrUrlRelation> {
    if let Some(TelegramLink::Message {
        peer: TelegramPeer::Username(username),
        ..
    }) = parse_telegram_link(target)
    {
        return Ok(match resolve_telegram_username(pool, &username).await? {
            TelegramUsernameResolution::Unique(peer)
                if matches!(peer.peer_type.as_str(), "channel" | "group" | "supergroup")
                    && (peer.peer_id == chat_id || affiliated_channels.contains(&peer.peer_id)) =>
            {
                QrUrlRelation::Affiliated
            }
            TelegramUsernameResolution::Unique(peer)
                if matches!(peer.peer_type.as_str(), "channel" | "group" | "supergroup") =>
            {
                QrUrlRelation::External
            }
            _ => QrUrlRelation::Unresolved,
        });
    }
    Ok(classify_qr_url(target, chat_id, affiliated_channels))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

pub fn relation(link: &TelegramLink, context: &RelationContext<'_>) -> LinkRelation {
    domain::relation(link, context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_are_candidate_only_and_require_authoritative_current_profile() {
        assert_eq!(normalize_username("@AliceBot"), Some("alicebot".to_owned()));
        assert!(is_bot_candidate("alicebot"));
        assert!(is_bot_candidate("aliceagent"));
        assert!(is_bot_candidate("aliceai"));
        assert!(is_bot_candidate("aliceapp"));
        assert!(is_bot_candidate("alicegame"));
        assert!(is_bot_candidate("alicegay"));
        assert!(!is_bot_candidate("alice"));
        assert_eq!(normalize_username("@short"), Some("short".to_owned()));
        assert_eq!(normalize_username("@not-valid!"), None);
    }

    #[test]
    fn relationship_target_extraction_keeps_native_text_mention_identity() {
        let message = serde_json::json!({
            "message_id": 7,
            "date": 1_700_000_000,
            "chat": {"id": -1001, "type": "supergroup"},
            "from": {"id": 9, "is_bot": false, "first_name": "Sender"},
            "text": "Alice",
            "entities": [{
                "type": "text_mention", "offset": 0, "length": 5,
                "user": {"id": 42, "is_bot": false, "first_name": "Alice"}
            }]
        });
        let targets = relation_targets(message, Some("Alice"));

        assert!(targets.iter().any(|target| {
            target.kind == Kind::UserId(42) && target.source == Source::TelegramEntity
        }));
    }

    #[test]
    fn rich_message_user_mentions_are_native_provenance() {
        assert!(is_native_user_mention(Source::RichMessage));
        assert!(is_native_user_mention(Source::TelegramEntity));
        assert!(!is_native_user_mention(Source::VisibleText));
    }

    #[test]
    fn qr_telegram_links_distinguish_affiliated_and_external_chats() {
        let affiliated = HashSet::from([-1_000_000_000_001]);
        assert_eq!(
            classify_qr_url("https://t.me/c/1/456", -1001, &affiliated),
            QrUrlRelation::Affiliated
        );
        assert_eq!(
            classify_qr_url("https://t.me/c/2/456", -1001, &affiliated),
            QrUrlRelation::External
        );
        assert_eq!(
            classify_qr_url("https://example.com/promo", -1001, &affiliated),
            QrUrlRelation::External
        );
    }

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

    #[tokio::test]
    #[ignore = "requires a disposable migrated PostgreSQL database"]
    async fn current_username_aliases_resolve_only_fresh_unique_owners() {
        const OWNER_A: i64 = 9_100_000_000_001;
        const OWNER_B: i64 = 9_100_000_000_002;
        const OWNER_C: i64 = 9_100_000_000_003;
        const OWNER_D: i64 = 9_100_000_000_004;
        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must point at a disposable test database");
        let pool = PgPool::connect(&database_url).await.unwrap();
        crate::db::migrate(&pool).await.unwrap();
        let readonly_role_exists: bool = sqlx::query_scalar(
            "select exists(select 1 from pg_roles where rolname = 'nedobot_mcp_ro')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if readonly_role_exists {
            let can_read_aliases: bool = sqlx::query_scalar(
                "select has_table_privilege('nedobot_mcp_ro', 'public.telegram_username_aliases', 'select')",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(
                !can_read_aliases,
                "private alias index must stay outside MCP read access"
            );
        }
        sqlx::query(
            "delete from public.telegram_username_aliases where telegram_peer_id = any($1)",
        )
        .bind([OWNER_A, OWNER_B, OWNER_C, OWNER_D])
        .execute(&pool)
        .await
        .unwrap();

        let first_user: teloxide::types::User = serde_json::from_value(serde_json::json!({
            "id": OWNER_A,
            "is_bot": false,
            "first_name": "Synthetic",
            "username": "OldName"
        }))
        .unwrap();
        crate::db::telegram::upsert_user_profile(&pool, &first_user)
            .await
            .unwrap();
        replace_current_telegram_username_aliases(
            &pool,
            OWNER_A,
            TelegramPeerType::User,
            "profile_refresh",
            &["OldName".to_owned(), "ExtraName".to_owned()],
        )
        .await
        .unwrap();
        crate::db::telegram::upsert_user_profile(&pool, &first_user)
            .await
            .unwrap();
        let names = vec!["oldname".to_owned(), "extraname".to_owned()];
        let resolved = resolve_current_username_aliases(&pool, &names)
            .await
            .unwrap();
        assert!(matches!(
            resolved.get("oldname"),
            Some(TelegramUsernameResolution::Unique(peer))
                if peer.peer_id == OWNER_A && peer.peer_type == "user"
        ));
        assert!(matches!(
            resolved.get("extraname"),
            Some(TelegramUsernameResolution::Unique(peer))
                if peer.peer_id == OWNER_A && peer.peer_type == "user"
        ));

        let renamed_user: teloxide::types::User = serde_json::from_value(serde_json::json!({
            "id": OWNER_A,
            "is_bot": false,
            "first_name": "Synthetic",
            "username": "CurrentName"
        }))
        .unwrap();
        crate::db::telegram::upsert_user_profile(&pool, &renamed_user)
            .await
            .unwrap();
        let names = vec![
            "oldname".to_owned(),
            "extraname".to_owned(),
            "currentname".to_owned(),
        ];
        let resolved = resolve_current_username_aliases(&pool, &names)
            .await
            .unwrap();
        assert!(!resolved.contains_key("oldname"));
        assert!(!resolved.contains_key("extraname"));
        assert!(matches!(
            resolved.get("currentname"),
            Some(TelegramUsernameResolution::Unique(peer))
                if peer.peer_id == OWNER_A && peer.peer_type == "user"
        ));

        for (owner_id, usernames) in [
            (OWNER_B, vec!["SharedAlias", "AffiliatedChannel"]),
            (OWNER_C, vec!["SharedAlias", "OutsideChannel"]),
        ] {
            replace_current_telegram_username_aliases(
                &pool,
                owner_id,
                TelegramPeerType::Channel,
                "configured_chat_refresh",
                &usernames.into_iter().map(str::to_owned).collect::<Vec<_>>(),
            )
            .await
            .unwrap();
        }
        replace_current_telegram_username_aliases(
            &pool,
            OWNER_D,
            TelegramPeerType::Bot,
            "known_bot_refresh",
            &["PrimaryBot".to_owned(), "ExtraBotName".to_owned()],
        )
        .await
        .unwrap();
        for username in ["primarybot", "extrabotname"] {
            assert_eq!(
                resolve_telegram_username(&pool, username).await.unwrap(),
                TelegramUsernameResolution::Unique(ResolvedTelegramPeer {
                    peer_id: OWNER_D,
                    peer_type: "bot".to_owned(),
                })
            );
        }
        assert_eq!(
            resolve_telegram_username(&pool, "@SharedAlias")
                .await
                .unwrap(),
            TelegramUsernameResolution::Ambiguous,
            "ambiguous aliases must fail closed"
        );
        let affiliated = HashSet::from([OWNER_B]);
        assert_eq!(
            classify_qr_url_with_aliases(
                &pool,
                "https://t.me/AffiliatedChannel/123",
                -1001,
                &affiliated
            )
            .await
            .unwrap(),
            QrUrlRelation::Affiliated
        );
        assert_eq!(
            classify_qr_url_with_aliases(
                &pool,
                "https://t.me/OutsideChannel/123",
                -1001,
                &affiliated
            )
            .await
            .unwrap(),
            QrUrlRelation::External
        );
        assert_eq!(
            classify_qr_url_with_aliases(
                &pool,
                "https://t.me/UnknownChannel/123",
                -1001,
                &affiliated
            )
            .await
            .unwrap(),
            QrUrlRelation::Unresolved
        );
        replace_current_telegram_username_aliases(
            &pool,
            OWNER_B,
            TelegramPeerType::Channel,
            "configured_chat_refresh",
            &["ReusedAlias".to_owned()],
        )
        .await
        .unwrap();
        replace_current_telegram_username_aliases(
            &pool,
            OWNER_B,
            TelegramPeerType::Channel,
            "configured_chat_refresh",
            &["FormerAlias".to_owned()],
        )
        .await
        .unwrap();
        replace_current_telegram_username_aliases(
            &pool,
            OWNER_C,
            TelegramPeerType::Channel,
            "configured_chat_refresh",
            &["ReusedAlias".to_owned()],
        )
        .await
        .unwrap();
        assert_eq!(
            resolve_telegram_username(&pool, "ReusedAlias")
                .await
                .unwrap(),
            TelegramUsernameResolution::Unique(ResolvedTelegramPeer {
                peer_id: OWNER_C,
                peer_type: "channel".to_owned(),
            })
        );

        replace_current_telegram_username_aliases(
            &pool,
            OWNER_B,
            TelegramPeerType::Channel,
            "configured_chat_refresh",
            &["StaleAlias".to_owned()],
        )
        .await
        .unwrap();
        sqlx::query("update public.telegram_username_aliases set observed_at = now() - interval '25 hours' where telegram_peer_id = $1")
            .bind(OWNER_B)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            resolve_current_username_aliases(&pool, &["stalealias".to_owned()])
                .await
                .unwrap()
                .is_empty(),
            "stale aliases must not be treated as current"
        );

        sqlx::query(
            "delete from public.telegram_username_aliases where telegram_peer_id = any($1)",
        )
        .bind([OWNER_A, OWNER_B, OWNER_C, OWNER_D])
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("delete from public.telegram_user_profiles where telegram_user_id = $1")
            .bind(OWNER_A)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }
}

mod domain;
