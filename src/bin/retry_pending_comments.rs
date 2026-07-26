use anyhow::Context;
use teloxide::{prelude::*, types::ParseMode};
use tg_ai_bot_teloxide::{
    config::Config,
    db::{build_pool, migrate},
    features::first_comment::pipeline::process_next_post_comment_job,
    state::AppState,
};

#[derive(Debug)]
struct Args {
    limit: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let args = parse_args()?;
    let config = Config::from_env()?;
    config.validate_runtime_secrets()?;
    let pool = build_pool().await?;
    migrate(&pool).await?;
    let state = AppState::new(pool, config);
    let bot = Bot::from_env().parse_mode(ParseMode::Html);

    let mut processed = 0;
    while processed < args.limit && process_next_post_comment_job(&bot, &state).await? {
        processed += 1;
    }
    println!("processed comment jobs: {processed}");

    Ok(())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut limit = 10usize;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--limit" => {
                limit = args
                    .next()
                    .context("--limit requires value")?
                    .parse()
                    .context("invalid --limit")?;
            }
            "-h" | "--help" => {
                println!("Usage: retry_pending_comments [--limit 10]");
                return Ok(Args { limit: 0 });
            }
            _ => anyhow::bail!("unknown option: {arg}"),
        }
    }

    Ok(Args { limit })
}
