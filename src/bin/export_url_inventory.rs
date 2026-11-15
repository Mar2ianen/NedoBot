use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufWriter, Write},
};

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use teloxide::types::Message;
use teloxide_url::{Kind, TelegramLink, parse_telegram_link};
use tg_ai_bot_teloxide::db::build_pool;

const PAGE_SIZE: i64 = 500;
const MAX_DISTINCT_TARGETS: usize = 100_000;
const MAX_TARGET_MESSAGE_PAIRS: usize = 1_000_000;

#[derive(Serialize)]
struct LinkRecord {
    record_type: &'static str,
    chat_id: i64,
    target: String,
    target_type: &'static str,
    resolution: &'static str,
    host: Option<String>,
    message_count: u64,
    unique_authors: usize,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    sources: BTreeSet<String>,
    spam_authors: usize,
}

#[derive(Default)]
struct Aggregate {
    message_ids: BTreeSet<i32>,
    authors: BTreeSet<i64>,
    spam_authors: BTreeSet<i64>,
    first_seen: Option<DateTime<Utc>>,
    last_seen: Option<DateTime<Utc>>,
    sources: BTreeSet<String>,
    host: Option<String>,
    target_type: &'static str,
}

#[derive(Default)]
struct HostAggregate {
    message_ids: BTreeSet<i32>,
    authors: BTreeSet<i64>,
    spam_authors: BTreeSet<i64>,
    first_seen: Option<DateTime<Utc>>,
    last_seen: Option<DateTime<Utc>>,
    sources: BTreeSet<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let mut args = std::env::args().skip(1);
    let mut chat_id = None;
    let mut output = None;
    let mut format = None;
    let mut redact_secrets = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--chat-id" => {
                chat_id = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--chat-id needs a value"))?
                        .parse::<i64>()?,
                )
            }
            "--output" => {
                output = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--output needs a path"))?,
                )
            }
            "--format" => {
                format = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--format needs a value"))?,
                )
            }
            "--redact-secrets" => redact_secrets = true,
            _ => anyhow::bail!("unknown argument: {arg}"),
        }
    }
    let chat_id = chat_id.ok_or_else(|| anyhow::anyhow!("usage: export_url_inventory --chat-id <CHAT_ID> --format jsonl --redact-secrets --output links.jsonl"))?;
    anyhow::ensure!(
        format.as_deref() == Some("jsonl"),
        "--format jsonl is required"
    );
    anyhow::ensure!(
        redact_secrets,
        "--redact-secrets is required; raw invite/auth parameters are never exported"
    );
    let output = output.ok_or_else(|| anyhow::anyhow!("--output is required"))?;
    let pool = build_pool().await?;
    export(&pool, chat_id, &output).await
}

async fn export(pool: &PgPool, chat_id: i64, output: &str) -> anyhow::Result<()> {
    let mut writer = BufWriter::new(File::create(output)?);
    let mut aggregates: BTreeMap<String, Aggregate> = BTreeMap::new();
    let mut target_message_pairs = 0usize;
    let mut last_id = i64::MIN;
    let mut unreadable = 0_u64;
    loop {
        let rows = sqlx::query(
            r#"select m.id, m.message_id, m.user_id, m.created_at, m.raw_json,
                      coalesce((select array_agg(e.target_sha256::text)
                                  from public.telegram_message_url_evidence e
                                 where e.chat_id = m.chat_id and e.message_id = m.message_id
                                   and e.source = 'qr' and e.parser_version = $4), array[]::text[]) as qr_hashes,
                      (u.spam_type is not null) as known_spam
                 from public.telegram_messages m
                 left join public.telegram_chat_users u
                   on u.chat_id = m.chat_id and u.telegram_user_id = m.user_id
                where m.chat_id = $1 and m.id > $2
                order by m.id limit $3"#,
        )
        .bind(chat_id)
        .bind(last_id)
        .bind(PAGE_SIZE)
        .bind(tg_ai_bot_teloxide::db::url_evidence::URL_EVIDENCE_PARSER_VERSION)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let db_id: i64 = row.try_get("id")?;
            last_id = db_id;
            let message_id: i32 = row.try_get("message_id")?;
            let user_id: Option<i64> = row.try_get("user_id")?;
            let created_at: DateTime<Utc> = row.try_get("created_at")?;
            let raw_json: serde_json::Value = row.try_get("raw_json")?;
            let qr_hashes: Vec<String> = row.try_get("qr_hashes")?;
            let known_spam: bool = row.try_get("known_spam")?;
            for hash in qr_hashes {
                let key = format!("qr_unrecoverable\0sha256:{hash}");
                if !aggregates.contains_key(&key) && aggregates.len() >= MAX_DISTINCT_TARGETS {
                    anyhow::bail!(
                        "distinct target limit ({MAX_DISTINCT_TARGETS}) reached; no partial inventory was written"
                    );
                }
                let entry = aggregates.entry(key).or_default();
                if entry.message_ids.insert(message_id) {
                    target_message_pairs += 1;
                    anyhow::ensure!(
                        target_message_pairs <= MAX_TARGET_MESSAGE_PAIRS,
                        "target/message pair limit ({MAX_TARGET_MESSAGE_PAIRS}) reached; no partial inventory was written"
                    );
                }
                entry.first_seen = Some(
                    entry
                        .first_seen
                        .map_or(created_at, |value| value.min(created_at)),
                );
                entry.last_seen = Some(
                    entry
                        .last_seen
                        .map_or(created_at, |value| value.max(created_at)),
                );
                if let Some(user_id) = user_id {
                    entry.authors.insert(user_id);
                    if known_spam {
                        entry.spam_authors.insert(user_id);
                    }
                }
                entry.sources.insert("qr_hash_only".to_owned());
                entry.target_type = "qr_target_unrecoverable";
            }
            let message: Message = match serde_json::from_value(raw_json) {
                Ok(message) => message,
                Err(_) => {
                    unreadable += 1;
                    continue;
                }
            };
            for found in teloxide_url::adapter::extract_from_message(&message) {
                let parsed = teloxide_url::parse_url(&found.value);
                let host = parsed
                    .as_ref()
                    .and_then(|url| url.host_str())
                    .map(str::to_owned);
                let kind = match found.kind {
                    Kind::Url => "url",
                    Kind::Mention => "telegram_username",
                    Kind::UserId(_) => "telegram_user_id",
                    _ => continue,
                };
                let target = match found.kind {
                    Kind::Url => match redact_target(&found.value) {
                        Some(target) => target,
                        None => continue,
                    },
                    Kind::Mention => {
                        format!("@{}", found.value.trim_start_matches('@').to_lowercase())
                    }
                    Kind::UserId(id) => format!("telegram-user-id:{id}"),
                    _ => continue,
                };
                let key = format!("{kind}\0{target}");
                if !aggregates.contains_key(&key) && aggregates.len() >= MAX_DISTINCT_TARGETS {
                    anyhow::bail!(
                        "distinct target limit ({MAX_DISTINCT_TARGETS}) reached; no partial inventory was written"
                    );
                }
                let entry = aggregates.entry(key).or_default();
                if entry.message_ids.insert(message_id) {
                    target_message_pairs += 1;
                    anyhow::ensure!(
                        target_message_pairs <= MAX_TARGET_MESSAGE_PAIRS,
                        "target/message pair limit ({MAX_TARGET_MESSAGE_PAIRS}) reached; no partial inventory was written"
                    );
                }
                entry.first_seen = Some(
                    entry
                        .first_seen
                        .map_or(created_at, |value| value.min(created_at)),
                );
                entry.last_seen = Some(
                    entry
                        .last_seen
                        .map_or(created_at, |value| value.max(created_at)),
                );
                if let Some(user_id) = user_id {
                    entry.authors.insert(user_id);
                    if known_spam {
                        entry.spam_authors.insert(user_id);
                    }
                }
                entry
                    .sources
                    .insert(format!("{:?}", found.source).to_lowercase());
                entry.host = host;
                entry.target_type = if kind == "url" && parse_telegram_link(&found.value).is_some()
                {
                    "telegram_target"
                } else {
                    kind
                };
            }
        }
        if rows.len() < PAGE_SIZE as usize {
            break;
        }
    }
    let mut hosts = BTreeMap::<String, HostAggregate>::new();
    let mut host_message_pairs = 0usize;
    for aggregate in aggregates.values() {
        let Some(host) = aggregate.host.as_ref() else {
            continue;
        };
        if !hosts.contains_key(host) && hosts.len() >= 25_000 {
            anyhow::bail!("host aggregation limit reached; no partial inventory was written");
        }
        let summary = hosts.entry(host.clone()).or_default();
        for message_id in &aggregate.message_ids {
            if summary.message_ids.insert(*message_id) {
                host_message_pairs += 1;
                anyhow::ensure!(
                    host_message_pairs <= MAX_TARGET_MESSAGE_PAIRS,
                    "host/message pair limit reached; no partial inventory was written"
                );
            }
        }
        summary.authors.extend(aggregate.authors.iter().copied());
        summary
            .spam_authors
            .extend(aggregate.spam_authors.iter().copied());
        summary.sources.extend(aggregate.sources.iter().cloned());
        if let Some(first) = aggregate.first_seen {
            summary.first_seen = Some(summary.first_seen.map_or(first, |value| value.min(first)));
        }
        if let Some(last) = aggregate.last_seen {
            summary.last_seen = Some(summary.last_seen.map_or(last, |value| value.max(last)));
        }
    }
    for (key, aggregate) in aggregates {
        let (kind, target) = key.split_once('\0').expect("internal key separator");
        let first_seen = aggregate.first_seen.unwrap_or_else(Utc::now);
        let last_seen = aggregate.last_seen.unwrap_or(first_seen);
        let fallback_type = match kind {
            "url" => "url",
            "telegram_username" => "telegram_username",
            "telegram_user_id" => "telegram_user_id",
            "qr_unrecoverable" => "qr_target_unrecoverable",
            _ => "unknown",
        };
        let record = LinkRecord {
            record_type: "target",
            chat_id,
            target: target.to_owned(),
            target_type: if aggregate.target_type.is_empty() {
                fallback_type
            } else {
                aggregate.target_type
            },
            resolution: "Unresolved",
            host: aggregate.host,
            message_count: aggregate.message_ids.len() as u64,
            unique_authors: aggregate.authors.len(),
            first_seen,
            last_seen,
            sources: aggregate.sources,
            spam_authors: aggregate.spam_authors.len(),
        };
        serde_json::to_writer(&mut writer, &record)?;
        writer.write_all(b"\n")?;
    }
    for (host, aggregate) in hosts {
        let first_seen = aggregate.first_seen.unwrap_or_else(Utc::now);
        let record = LinkRecord {
            record_type: "host_summary",
            chat_id,
            target: host.clone(),
            target_type: "host",
            resolution: "not_applicable",
            host: Some(host),
            message_count: aggregate.message_ids.len() as u64,
            unique_authors: aggregate.authors.len(),
            first_seen,
            last_seen: aggregate.last_seen.unwrap_or(first_seen),
            sources: aggregate.sources,
            spam_authors: aggregate.spam_authors.len(),
        };
        serde_json::to_writer(&mut writer, &record)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    eprintln!("chat_id={chat_id} unreadable_messages={unreadable} output={output}");
    Ok(())
}

fn redact_target(value: &str) -> Option<String> {
    match parse_telegram_link(value) {
        Some(TelegramLink::Invite { .. } | TelegramLink::AmbiguousPlus { .. }) => {
            return Some("https://t.me/<redacted-invite>".to_owned());
        }
        Some(TelegramLink::BotStart { username, kind, .. }) => {
            let key = match kind {
                teloxide_url::BotStartKind::Start => "start",
                teloxide_url::BotStartKind::StartGroup => "startgroup",
                teloxide_url::BotStartKind::StartChannel => "startchannel",
                teloxide_url::BotStartKind::StartApp => "startapp",
            };
            return Some(format!("https://t.me/{username}?{key}=<redacted>"));
        }
        _ => {}
    }
    let mut url = teloxide_url::parse_url(value)?;
    url.set_username("").ok()?;
    url.set_password(None).ok()?;
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().to_owned();
    let segments = path.split('/').collect::<Vec<_>>();
    let sensitive_route = ["login", "auth", "oauth", "reset", "verify", "invite"];
    if let Some(index) = segments.iter().position(|segment| {
        sensitive_route
            .iter()
            .any(|route| segment.eq_ignore_ascii_case(route))
    }) {
        let safe_prefix = segments[..=index].join("/");
        url.set_path(&format!("{safe_prefix}/<redacted>"));
    } else if segments.iter().any(|segment| segment.len() > 48) {
        url.set_path("/<redacted-path>");
    }
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::redact_target;

    #[test]
    fn inventory_redacts_auth_query_and_invite_secrets() {
        assert_eq!(
            redact_target("https://example.org/review?token=secret#auth").as_deref(),
            Some("https://example.org/review")
        );
        assert_eq!(
            redact_target("https://t.me/+SyntheticInviteHash").as_deref(),
            Some("https://t.me/<redacted-invite>")
        );
        assert_eq!(
            redact_target("https://t.me/review_bot?start=secret").as_deref(),
            Some("https://t.me/review_bot?start=<redacted>")
        );
        assert_eq!(
            redact_target("https://example.org/auth/shortsecret/continue").as_deref(),
            Some("https://example.org/auth/%3Credacted%3E")
        );
    }
}
