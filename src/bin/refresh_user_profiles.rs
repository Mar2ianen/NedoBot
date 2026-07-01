use anyhow::{Context, bail};
use std::time::Duration;
use teloxide::prelude::*;
use tg_ai_bot_teloxide::{
    config::Config,
    db::{build_pool, migrate},
    features::user_profiles::service::{RefreshUserProfilesQuery, load_user_ids, refresh_profile},
};
use tokio::time::sleep;

#[derive(Debug)]
struct Args {
    chat_id: Option<i64>,
    limit: i64,
    sleep_ms: u64,
    all: bool,
    only_spammers: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let args = parse_args()?;
    let config = Config::from_env();
    let chat_id = args.chat_id.unwrap_or(config.discussion_chat_id);
    let pool = build_pool().await?;
    migrate(&pool).await?;
    let bot = Bot::from_env();

    let user_ids = load_user_ids(
        &pool,
        &RefreshUserProfilesQuery {
            chat_id,
            limit: args.limit,
            include_refreshed: args.all,
            only_spammers: args.only_spammers,
        },
    )
    .await?;
    println!(
        "refresh user profiles: chat_id={} users={} sleep_ms={} mode={}",
        chat_id,
        user_ids.len(),
        args.sleep_ms,
        if args.only_spammers {
            "spammers"
        } else if args.all {
            "all"
        } else {
            "missing"
        }
    );

    let mut refreshed = 0usize;
    let mut failed = 0usize;
    for (index, user_id) in user_ids.iter().copied().enumerate() {
        match refresh_profile(&bot, &pool, user_id).await {
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
    let mut limit = 200i64;
    let mut sleep_ms = 100u64;
    let mut all = false;
    let mut only_spammers = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--all" => all = true,
            "--only-spammers" => only_spammers = true,
            "--chat-id" => {
                chat_id = Some(
                    args.next()
                        .context("--chat-id requires value")?
                        .parse()
                        .context("invalid --chat-id")?,
                );
            }
            "--limit" => {
                limit = args
                    .next()
                    .context("--limit requires value")?
                    .parse()
                    .context("invalid --limit")?;
            }
            "--sleep-ms" => {
                sleep_ms = args
                    .next()
                    .context("--sleep-ms requires value")?
                    .parse()
                    .context("invalid --sleep-ms")?;
            }
            "-h" | "--help" => {
                println!(
                    "Usage: refresh_user_profiles [--all|--only-spammers] [--chat-id -100...] [--limit 200] [--sleep-ms 100]"
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
        only_spammers,
    })
}
