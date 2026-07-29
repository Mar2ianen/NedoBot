use sqlx::{PgPool, postgres::PgPoolOptions};

pub mod telegram;

pub async fn build_pool() -> anyhow::Result<PgPool> {
    let database_url = std::env::var("DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .map_err(|_| anyhow::anyhow!("database connection failed"))?;

    Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    // Keep this macro adjacent to migrations so sqlx recompiles embedded migration changes.
    // Touched with each migration addition because SQLx embeds this directory at compile time.
    // The job lifecycle observability migration is embedded with this directory.
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
