use tg_ai_bot_teloxide::db::{build_pool, migrate};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let pool = build_pool().await?;
    migrate(&pool).await?;
    println!("database migrations applied");
    Ok(())
}
