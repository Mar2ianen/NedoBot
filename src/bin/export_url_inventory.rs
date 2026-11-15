use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
};

use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use teloxide::types::Message;
use teloxide_url::{Kind, TelegramLink, parse_telegram_link};
use tempfile::NamedTempFile;
use tg_ai_bot_teloxide::db::build_pool;

const PAGE_SIZE: i64 = 500;
const MAX_DISTINCT_TARGETS: usize = 100_000;
const MAX_TARGET_MESSAGE_PAIRS: usize = 1_000_000;

#[derive(Serialize)]
struct LinkRecord {
    record_type: &'static str,
    chat_id: i64,
    target: String,
    identity_sha256: String,
    target_type: &'static str,
    resolution: &'static str,
    host: Option<String>,
    message_count: u64,
    unique_authors: usize,
    currently_flagged_spam_authors: usize,
    operator_confirmed_spam_authors: usize,
    operator_confirmed_normal_authors: usize,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    sources: BTreeSet<String>,
}

#[derive(Default)]
struct Aggregate {
    message_ids: BTreeSet<i32>,
    authors: BTreeSet<i64>,
    currently_flagged_spam_authors: BTreeSet<i64>,
    operator_confirmed_spam_authors: BTreeSet<i64>,
    operator_confirmed_normal_authors: BTreeSet<i64>,
    first_seen: Option<DateTime<Utc>>,
    last_seen: Option<DateTime<Utc>>,
    sources: BTreeSet<String>,
    host: Option<String>,
    target_type: &'static str,
    display_target: String,
    identity_sha256: String,
}

#[derive(Default)]
struct HostAggregate {
    message_ids: BTreeSet<i32>,
    authors: BTreeSet<i64>,
    currently_flagged_spam_authors: BTreeSet<i64>,
    operator_confirmed_spam_authors: BTreeSet<i64>,
    operator_confirmed_normal_authors: BTreeSet<i64>,
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
    let mut show_paths = false;
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
            "--show-paths" => show_paths = true,
            _ => anyhow::bail!("unknown argument: {arg}"),
        }
    }
    let chat_id = chat_id.ok_or_else(|| anyhow::anyhow!("usage: export_url_inventory --chat-id <CHAT_ID> --format jsonl --redact-secrets [--show-paths] --output links.jsonl"))?;
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
    export(&pool, chat_id, &output, show_paths).await
}

async fn export(pool: &PgPool, chat_id: i64, output: &str, show_paths: bool) -> anyhow::Result<()> {
    let output_path = Path::new(output);
    let (mut temporary, parent) = create_private_output_temp(output_path)?;
    let mut writer = BufWriter::new(temporary.as_file_mut());
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
                      coalesce(u.is_spammer, false) as currently_flagged_spam,
                      human_label.label as latest_operator_label
                 from public.telegram_messages m
                 left join public.telegram_chat_users u
                   on u.chat_id = m.chat_id and u.telegram_user_id = m.user_id
                 left join lateral (
                    select e.label
                      from public.spam_label_events e
                     where e.chat_id = m.chat_id
                       and e.telegram_user_id = m.user_id
                       and e.source in ('owner_review', 'owner_manual', 'legacy_backfill')
                     order by e.created_at desc, e.id desc
                     limit 1
                 ) human_label on true
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
            let currently_flagged_spam: bool = row.try_get("currently_flagged_spam")?;
            let latest_operator_label: Option<String> = row.try_get("latest_operator_label")?;
            let operator_confirmed_spam = latest_operator_label.as_deref() == Some("spam");
            let operator_confirmed_normal = latest_operator_label.as_deref() == Some("not_spam");
            for hash in qr_hashes {
                let target = format!("sha256:{hash}");
                let fingerprint = fingerprint(&format!("qr-hash-only:{hash}"));
                let key = format!("qr_unrecoverable\0{fingerprint}");
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
                    if currently_flagged_spam {
                        entry.currently_flagged_spam_authors.insert(user_id);
                    }
                    if operator_confirmed_spam {
                        entry.operator_confirmed_spam_authors.insert(user_id);
                    }
                    if operator_confirmed_normal {
                        entry.operator_confirmed_normal_authors.insert(user_id);
                    }
                }
                entry.sources.insert("qr_hash_only".to_owned());
                entry.target_type = "qr_target_unrecoverable";
                entry.display_target = target;
                entry.identity_sha256 = fingerprint;
            }
            let message: Message = match serde_json::from_value(raw_json) {
                Ok(message) => message,
                Err(_) => {
                    unreadable += 1;
                    continue;
                }
            };
            for found in teloxide_url::adapter::extract_from_message(&message) {
                let kind = match found.kind {
                    Kind::Url => "url",
                    Kind::Mention => "telegram_username",
                    Kind::UserId(_) => "telegram_user_id",
                    _ => continue,
                };
                let (identity, target, host) = match found.kind {
                    Kind::Url => {
                        let Some(parsed) = teloxide_url::parse_url(&found.value) else {
                            continue;
                        };
                        let identity = parsed.to_string();
                        let Some(target) = redact_target(&identity, show_paths) else {
                            continue;
                        };
                        let host = parsed.host_str().map(str::to_owned);
                        (identity, target, host)
                    }
                    Kind::Mention => {
                        let username = found.value.trim_start_matches('@').to_lowercase();
                        (format!("@{username}"), format!("@{username}"), None)
                    }
                    Kind::UserId(id) => {
                        let target = format!("telegram-user-id:{id}");
                        (target.clone(), target, None)
                    }
                    _ => continue,
                };
                let target_fingerprint = fingerprint(&identity);
                let key = format!("{kind}\0{target_fingerprint}");
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
                    if currently_flagged_spam {
                        entry.currently_flagged_spam_authors.insert(user_id);
                    }
                    if operator_confirmed_spam {
                        entry.operator_confirmed_spam_authors.insert(user_id);
                    }
                    if operator_confirmed_normal {
                        entry.operator_confirmed_normal_authors.insert(user_id);
                    }
                }
                entry
                    .sources
                    .insert(format!("{:?}", found.source).to_lowercase());
                entry.display_target = target;
                entry.identity_sha256 = target_fingerprint;
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
            .currently_flagged_spam_authors
            .extend(aggregate.currently_flagged_spam_authors.iter().copied());
        summary
            .operator_confirmed_spam_authors
            .extend(aggregate.operator_confirmed_spam_authors.iter().copied());
        summary
            .operator_confirmed_normal_authors
            .extend(aggregate.operator_confirmed_normal_authors.iter().copied());
        summary.sources.extend(aggregate.sources.iter().cloned());
        if let Some(first) = aggregate.first_seen {
            summary.first_seen = Some(summary.first_seen.map_or(first, |value| value.min(first)));
        }
        if let Some(last) = aggregate.last_seen {
            summary.last_seen = Some(summary.last_seen.map_or(last, |value| value.max(last)));
        }
    }
    for (key, aggregate) in aggregates {
        let (kind, _) = key.split_once('\0').expect("internal key separator");
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
            target: aggregate.display_target.clone(),
            identity_sha256: aggregate.identity_sha256.clone(),
            target_type: if aggregate.target_type.is_empty() {
                fallback_type
            } else {
                aggregate.target_type
            },
            resolution: "Unresolved",
            host: aggregate.host,
            message_count: aggregate.message_ids.len() as u64,
            unique_authors: aggregate.authors.len(),
            currently_flagged_spam_authors: aggregate.currently_flagged_spam_authors.len(),
            operator_confirmed_spam_authors: aggregate.operator_confirmed_spam_authors.len(),
            operator_confirmed_normal_authors: aggregate.operator_confirmed_normal_authors.len(),
            first_seen,
            last_seen,
            sources: aggregate.sources,
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
            identity_sha256: fingerprint(&host),
            target_type: "host",
            resolution: "not_applicable",
            host: Some(host),
            message_count: aggregate.message_ids.len() as u64,
            unique_authors: aggregate.authors.len(),
            currently_flagged_spam_authors: aggregate.currently_flagged_spam_authors.len(),
            operator_confirmed_spam_authors: aggregate.operator_confirmed_spam_authors.len(),
            operator_confirmed_normal_authors: aggregate.operator_confirmed_normal_authors.len(),
            first_seen,
            last_seen: aggregate.last_seen.unwrap_or(first_seen),
            sources: aggregate.sources,
        };
        serde_json::to_writer(&mut writer, &record)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    writer.get_ref().sync_all()?;
    drop(writer);
    persist_private_output(temporary, output_path, &parent)?;
    eprintln!("chat_id={chat_id} unreadable_messages={unreadable} output={output}");
    Ok(())
}

fn create_private_output_temp(
    output_path: &Path,
) -> anyhow::Result<(NamedTempFile, std::path::PathBuf)> {
    let parent = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let temporary = NamedTempFile::new_in(&parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok((temporary, parent))
}

fn persist_private_output(
    temporary: NamedTempFile,
    output_path: &Path,
    parent: &Path,
) -> anyhow::Result<()> {
    temporary.persist(output_path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn redact_target(value: &str, show_paths: bool) -> Option<String> {
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
    if !show_paths && url.path() != "/" {
        url.set_path("/<redacted-path>");
        return Some(url.to_string());
    }
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

fn fingerprint(identity: &str) -> String {
    Sha256::digest(identity.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{create_private_output_temp, fingerprint, persist_private_output, redact_target};
    use std::io::Write;

    #[test]
    fn inventory_redacts_auth_query_and_invite_secrets() {
        assert_eq!(
            redact_target("https://example.org/review?token=secret#auth", false).as_deref(),
            Some("https://example.org/%3Credacted-path%3E")
        );
        assert_eq!(
            redact_target("https://t.me/+SyntheticInviteHash", false).as_deref(),
            Some("https://t.me/<redacted-invite>")
        );
        assert_eq!(
            redact_target("https://t.me/review_bot?start=secret", false).as_deref(),
            Some("https://t.me/review_bot?start=<redacted>")
        );
        assert_eq!(
            redact_target("https://example.org/auth/shortsecret/continue", true).as_deref(),
            Some("https://example.org/auth/%3Credacted%3E")
        );
        assert_eq!(
            redact_target("https://example.org/r/secret123?campaign=secret", false).as_deref(),
            Some("https://example.org/%3Credacted-path%3E")
        );
        assert_eq!(
            redact_target("https://example.org/careers/apply?token=secret", true).as_deref(),
            Some("https://example.org/careers/apply")
        );
        assert_eq!(
            redact_target("https://example.org/?campaign=normal", false),
            redact_target("https://example.org/?campaign=spam", false)
        );
        assert_ne!(
            fingerprint("https://example.org/?campaign=normal"),
            fingerprint("https://example.org/?campaign=spam")
        );
        assert_ne!(
            fingerprint("https://example.org/File"),
            fingerprint("https://example.org/file")
        );
    }

    #[test]
    fn inventory_redacts_all_bot_start_payloads_and_invite_hashes() {
        let fixtures = [
            (
                "https://t.me/demo_bot?start=synthetic-token",
                "https://t.me/demo_bot?start=<redacted>",
            ),
            (
                "https://t.me/demo_bot?startgroup=synthetic-token",
                "https://t.me/demo_bot?startgroup=<redacted>",
            ),
            (
                "https://t.me/demo_bot?startchannel=synthetic-token",
                "https://t.me/demo_bot?startchannel=<redacted>",
            ),
            (
                "https://t.me/demo_bot/mini?startapp=synthetic-token",
                "https://t.me/demo_bot?startapp=<redacted>",
            ),
            (
                "tg://resolve?domain=demo_bot&startapp=synthetic-token",
                "https://t.me/demo_bot?startapp=<redacted>",
            ),
        ];

        for (input, expected) in fixtures {
            let redacted = redact_target(input, true).expect("Telegram deep link must parse");
            assert_eq!(redacted, expected);
            assert!(!redacted.contains("synthetic-token"));
        }

        for input in [
            "https://t.me/+SyntheticInviteHash",
            "https://t.me/joinchat/SyntheticInviteHash",
            "tg://join?invite=SyntheticInviteHash",
        ] {
            let redacted = redact_target(input, true).expect("invite link must parse");
            assert_eq!(redacted, "https://t.me/<redacted-invite>");
            assert!(!redacted.contains("SyntheticInviteHash"));
        }
    }

    #[tokio::test]
    #[ignore = "requires a disposable migrated PostgreSQL database"]
    async fn export_separates_current_flags_from_latest_operator_labels() {
        use super::export;
        use sqlx::PgPool;

        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must point at a disposable migrated PostgreSQL database");
        let pool = PgPool::connect(&database_url).await.unwrap();
        tg_ai_bot_teloxide::db::migrate(&pool).await.unwrap();
        const CHAT_ID: i64 = -100_991_229_901;
        const USER_SYSTEM_ONLY: i64 = 9_912_299_101;
        const USER_OPERATOR_NORMAL: i64 = 9_912_299_102;
        const USER_OPERATOR_SPAM: i64 = 9_912_299_103;
        const MESSAGE_BASE: i32 = 9_922_990;

        sqlx::query(
            "delete from telegram_messages where chat_id = $1 and message_id between $2 and $3",
        )
        .bind(CHAT_ID)
        .bind(MESSAGE_BASE)
        .bind(MESSAGE_BASE + 2)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "delete from spam_label_events where chat_id = $1 and telegram_user_id = any($2)",
        )
        .bind(CHAT_ID)
        .bind(vec![
            USER_SYSTEM_ONLY,
            USER_OPERATOR_NORMAL,
            USER_OPERATOR_SPAM,
        ])
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "delete from telegram_chat_users where chat_id = $1 and telegram_user_id = any($2)",
        )
        .bind(CHAT_ID)
        .bind(vec![
            USER_SYSTEM_ONLY,
            USER_OPERATOR_NORMAL,
            USER_OPERATOR_SPAM,
        ])
        .execute(&pool)
        .await
        .unwrap();

        for (user_id, is_spammer) in [
            (USER_SYSTEM_ONLY, false),
            (USER_OPERATOR_NORMAL, true),
            (USER_OPERATOR_SPAM, true),
        ] {
            sqlx::query("insert into telegram_chat_users(chat_id, telegram_user_id, first_seen_at, message_count, is_spammer, spam_type) values ($1, $2, now(), 1, $3, 'fixture_system_type')")
                .bind(CHAT_ID)
                .bind(user_id)
                .bind(is_spammer)
                .execute(&pool)
                .await
                .unwrap();
            let raw_json = serde_json::json!({
                "message_id": MESSAGE_BASE + (user_id - USER_SYSTEM_ONLY) as i32,
                "date": 1_700_000_000,
                "chat": {"id": CHAT_ID, "type": "supergroup"},
                "from": {"id": user_id, "is_bot": false, "first_name": "Fixture"},
                "text": "https://example.org/r/secret123"
            });
            sqlx::query("insert into telegram_messages(chat_id, message_id, user_id, text, raw_json, created_at) values ($1,$2,$3,$4,$5,now())")
                .bind(CHAT_ID)
                .bind(raw_json["message_id"].as_i64().unwrap() as i32)
                .bind(user_id)
                .bind("https://example.org/r/secret123")
                .bind(raw_json)
                .execute(&pool)
                .await
                .unwrap();
        }

        for (user_id, label, source) in [
            (USER_SYSTEM_ONLY, "spam", "system"),
            (USER_OPERATOR_NORMAL, "spam", "owner_review"),
            (USER_OPERATOR_NORMAL, "not_spam", "owner_review"),
            (USER_OPERATOR_NORMAL, "spam", "system"),
            (USER_OPERATOR_SPAM, "spam", "owner_manual"),
        ] {
            sqlx::query("insert into spam_label_events(chat_id, telegram_user_id, label, subtype, source, reason) values($1,$2,$3,'fixture',$4,'fixture label')")
                .bind(CHAT_ID)
                .bind(user_id)
                .bind(label)
                .bind(source)
                .execute(&pool)
                .await
                .unwrap();
        }

        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("inventory.jsonl");
        export(&pool, CHAT_ID, output.to_str().unwrap(), false)
            .await
            .unwrap();
        let content = std::fs::read_to_string(output).unwrap();
        let target: serde_json::Value = content
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .find(|record: &serde_json::Value| record["record_type"] == "target")
            .unwrap();
        assert_eq!(target["target"], "https://example.org/%3Credacted-path%3E");
        assert_eq!(target["unique_authors"], 3);
        assert_eq!(target["currently_flagged_spam_authors"], 2);
        assert_eq!(target["operator_confirmed_spam_authors"], 1);
        assert_eq!(target["operator_confirmed_normal_authors"], 1);

        sqlx::query(
            "delete from telegram_messages where chat_id = $1 and message_id between $2 and $3",
        )
        .bind(CHAT_ID)
        .bind(MESSAGE_BASE)
        .bind(MESSAGE_BASE + 2)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "delete from spam_label_events where chat_id = $1 and telegram_user_id = any($2)",
        )
        .bind(CHAT_ID)
        .bind(vec![
            USER_SYSTEM_ONLY,
            USER_OPERATOR_NORMAL,
            USER_OPERATOR_SPAM,
        ])
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "delete from telegram_chat_users where chat_id = $1 and telegram_user_id = any($2)",
        )
        .bind(CHAT_ID)
        .bind(vec![
            USER_SYSTEM_ONLY,
            USER_OPERATOR_NORMAL,
            USER_OPERATOR_SPAM,
        ])
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    }

    #[test]
    fn inventory_output_is_private_and_failed_runs_preserve_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("links.jsonl");
        std::fs::write(&output, "previous complete inventory\n").unwrap();
        {
            let (mut temporary, _) = create_private_output_temp(&output).unwrap();
            temporary.write_all(b"partial inventory").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    temporary.as_file().metadata().unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        assert_eq!(
            std::fs::read_to_string(&output).unwrap(),
            "previous complete inventory\n"
        );

        let (mut temporary, parent) = create_private_output_temp(&output).unwrap();
        temporary.write_all(b"complete inventory\n").unwrap();
        temporary.as_file().sync_all().unwrap();
        persist_private_output(temporary, &output, &parent).unwrap();
        assert_eq!(
            std::fs::read_to_string(&output).unwrap(),
            "complete inventory\n"
        );
        assert_eq!(parent, directory.path());
    }
}
