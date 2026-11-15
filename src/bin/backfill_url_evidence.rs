use tg_ai_bot_teloxide::db::{build_pool, url_evidence::backfill_url_evidence};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let mut args = std::env::args().skip(1);
    let chat_id = args
        .next()
        .ok_or_else(|| anyhow::anyhow!(
            "usage: backfill_url_evidence <chat-id> [--after <message-id>] [--limit <1..10000>] [--apply]"
        ))?
        .parse::<i64>()?;
    let mut after_message_id = 0_i32;
    let mut limit = 500_i64;
    let mut apply = false;
    while let Some(option) = args.next() {
        match option.as_str() {
            "--after" => {
                after_message_id = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--after needs a message ID"))?
                    .parse()?
            }
            "--limit" => {
                limit = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--limit needs a value"))?
                    .parse()?
            }
            "--apply" => apply = true,
            _ => anyhow::bail!("unknown option: {option}"),
        }
    }
    let pool = build_pool().await?;
    let summary = backfill_url_evidence(&pool, chat_id, after_message_id, limit, apply).await?;
    println!(
        "apply={apply} candidates={} materialized={} references={} unreadable={} next_after={}",
        summary.candidates,
        summary.materialized,
        summary.references,
        summary.unreadable,
        summary.last_message_id.unwrap_or(after_message_id)
    );
    Ok(())
}
