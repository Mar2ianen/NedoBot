use std::{path::PathBuf, time::Duration};

use anyhow::{Context, bail};
use sqlx::PgPool;
use teloxide::{
    prelude::*,
    types::{ChatMember, ChatMemberKind},
};
use tg_ai_bot_teloxide::{
    config::Config,
    db::{build_pool, migrate, telegram::upsert_user_profile},
};
use tokio::time::sleep;

#[derive(Debug)]
struct Args {
    chat_id: Option<i64>,
    limit: Option<i64>,
    sleep_ms: u64,
    all: bool,
    user_ids_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let args = parse_args()?;
    let config = Config::from_env();
    let chat_id = args.chat_id.unwrap_or(config.discussion_chat_id);
    let pool = build_pool().await?;
    migrate(&pool).await?;

    let user_ids = load_user_ids(&pool, chat_id, &args).await?;
    println!(
        "refresh chat members: chat_id={} users={} sleep_ms={} mode={}",
        chat_id,
        user_ids.len(),
        args.sleep_ms,
        if args.all { "all" } else { "missing usernames" }
    );

    let bot = Bot::from_env();
    let mut refreshed = 0usize;
    let mut failed = 0usize;

    for (index, user_id) in user_ids.iter().copied().enumerate() {
        match refresh_member(&bot, &pool, chat_id, user_id).await {
            Ok(()) => refreshed += 1,
            Err(err) => {
                failed += 1;
                println!("failed user_id={user_id}: {err:#}");
            }
        }

        if (index + 1) % 50 == 0 || index + 1 == user_ids.len() {
            println!(
                "progress: {}/{} refreshed={} failed={}",
                index + 1,
                user_ids.len(),
                refreshed,
                failed
            );
        }

        sleep(Duration::from_millis(args.sleep_ms)).await;
    }

    println!("done: refreshed={refreshed} failed={failed}");
    Ok(())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut chat_id = None;
    let mut limit = None;
    let mut sleep_ms = 80u64;
    let mut all = false;
    let mut user_ids_file = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--all" => all = true,
            "--chat-id" => {
                chat_id = Some(
                    args.next()
                        .context("--chat-id requires value")?
                        .parse()
                        .context("invalid --chat-id")?,
                );
            }
            "--limit" => {
                limit = Some(
                    args.next()
                        .context("--limit requires value")?
                        .parse()
                        .context("invalid --limit")?,
                );
            }
            "--sleep-ms" => {
                sleep_ms = args
                    .next()
                    .context("--sleep-ms requires value")?
                    .parse()
                    .context("invalid --sleep-ms")?;
            }
            "--user-ids-file" => {
                user_ids_file = Some(PathBuf::from(
                    args.next().context("--user-ids-file requires value")?,
                ));
            }
            "-h" | "--help" => {
                println!(
                    "Usage: refresh_chat_members [--all] [--chat-id -100...] [--limit N] [--sleep-ms 80] [--user-ids-file ids.txt]"
                );
                std::process::exit(0);
            }
            _ => bail!("unknown option: {arg}"),
        }
    }

    Ok(Args {
        chat_id,
        limit,
        sleep_ms,
        all,
        user_ids_file,
    })
}

async fn load_user_ids(pool: &PgPool, chat_id: i64, args: &Args) -> anyhow::Result<Vec<i64>> {
    if let Some(path) = args.user_ids_file.as_ref() {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        return Ok(content
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect());
    }

    let limit = args.limit.unwrap_or(i64::MAX);
    let rows = if args.all {
        sqlx::query_as::<_, (i64,)>(
            r#"
            select cu.telegram_user_id
            from telegram_chat_users cu
            where cu.chat_id = $1
            order by cu.last_seen_at desc nulls last
            limit $2
            "#,
        )
        .bind(chat_id)
        .bind(limit)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, (i64,)>(
            r#"
            select cu.telegram_user_id
            from telegram_chat_users cu
            left join telegram_user_profiles p on p.telegram_user_id = cu.telegram_user_id
            where cu.chat_id = $1
              and nullif(trim(coalesce(p.username, '')), '') is null
            order by cu.last_seen_at desc nulls last
            limit $2
            "#,
        )
        .bind(chat_id)
        .bind(limit)
        .fetch_all(pool)
        .await?
    };

    Ok(rows.into_iter().map(|(user_id,)| user_id).collect())
}

async fn refresh_member(
    bot: &Bot,
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
) -> anyhow::Result<()> {
    let user_id = u64::try_from(user_id).context("negative user id")?;
    let member = bot
        .get_chat_member(ChatId(chat_id), UserId(user_id))
        .await
        .context("getChatMember failed")?;

    upsert_user_profile(pool, &member.user).await?;
    upsert_member_snapshot(pool, chat_id, &member).await?;

    Ok(())
}

async fn upsert_member_snapshot(
    pool: &PgPool,
    chat_id: i64,
    member: &ChatMember,
) -> anyhow::Result<()> {
    let raw_json = serde_json::to_value(member)?;
    let (status, is_admin, is_present) = member_status(&member.kind);

    sqlx::query(
        r#"
        insert into telegram_chat_member_snapshots
            (chat_id, telegram_user_id, status, is_admin, is_present, raw_json, observed_at)
        values ($1, $2, $3, $4, $5, $6, now())
        on conflict (chat_id, telegram_user_id) do update set
            status = excluded.status,
            is_admin = excluded.is_admin,
            is_present = excluded.is_present,
            raw_json = excluded.raw_json,
            observed_at = excluded.observed_at
        "#,
    )
    .bind(chat_id)
    .bind(member.user.id.0 as i64)
    .bind(status)
    .bind(is_admin)
    .bind(is_present)
    .bind(raw_json)
    .execute(pool)
    .await?;

    Ok(())
}

fn member_status(kind: &ChatMemberKind) -> (&'static str, bool, bool) {
    match kind {
        ChatMemberKind::Owner(_) => ("creator", true, true),
        ChatMemberKind::Administrator(_) => ("administrator", true, true),
        ChatMemberKind::Member => ("member", false, true),
        ChatMemberKind::Restricted(restricted) => ("restricted", false, restricted.is_member),
        ChatMemberKind::Left => ("left", false, false),
        ChatMemberKind::Banned(_) => ("banned", false, false),
    }
}
