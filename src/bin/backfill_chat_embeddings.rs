use std::time::Duration;

use anyhow::{Context, bail};
use tg_ai_bot_teloxide::{
    config::Config,
    db::{build_pool, migrate},
    features::chat_retrieval::enqueue_backfill_batch,
};
use tokio::time::sleep;

#[derive(Debug)]
struct Args {
    chat_id: Option<i64>,
    batch_size: i64,
    sleep_ms: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let args = parse_args()?;
    let config = Config::from_env();
    let chat_id = args.chat_id.unwrap_or(config.discussion_chat_id);
    let pool = build_pool().await?;
    migrate(&pool).await?;

    let mut queued = 0usize;
    loop {
        let inserted = enqueue_backfill_batch(&pool, chat_id, args.batch_size).await?;
        queued += inserted;
        println!("queued={queued} last_batch={inserted}");
        if inserted == 0 {
            break;
        }
        sleep(Duration::from_millis(args.sleep_ms)).await;
    }
    Ok(())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut chat_id = None;
    let mut batch_size = 200;
    let mut sleep_ms = 250;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--chat-id" => {
                chat_id = Some(args.next().context("--chat-id requires value")?.parse()?);
            }
            "--batch-size" => {
                batch_size = args
                    .next()
                    .context("--batch-size requires value")?
                    .parse()
                    .context("invalid --batch-size")?;
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
                    "Usage: backfill_chat_embeddings [--chat-id -100...] [--batch-size 200] [--sleep-ms 250]"
                );
                std::process::exit(0);
            }
            _ => bail!("unknown option: {arg}"),
        }
    }
    Ok(Args {
        chat_id,
        batch_size: batch_size.clamp(1, 1_000),
        sleep_ms,
    })
}
