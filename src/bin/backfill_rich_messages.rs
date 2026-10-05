use tg_ai_bot_teloxide::db::{build_pool, rich_backfill::backfill_rich_messages};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let mut args = std::env::args().skip(1);
    let chat_id = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: backfill_rich_messages <chat-id> [--apply]"))?
        .parse::<i64>()?;
    let apply = match args.next().as_deref() {
        None => false,
        Some("--apply") => true,
        Some(_) => anyhow::bail!("only --apply is supported; omit it for a dry run"),
    };
    if args.next().is_some() {
        anyhow::bail!("unexpected arguments");
    }
    let pool = build_pool().await?;
    let summary = backfill_rich_messages(&pool, chat_id, apply).await?;
    println!(
        "apply={apply} candidates={} repaired={} unreadable={}",
        summary.candidates, summary.repaired, summary.unreadable
    );
    Ok(())
}
