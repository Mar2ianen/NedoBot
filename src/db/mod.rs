use sqlx::{PgPool, postgres::PgPoolOptions};
use std::collections::HashSet;

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
    // Job observability and moderation operation-intent migrations are embedded here.
    const KNOWN_UNRESOLVED_PRODUCTION_MIGRATIONS: &[i64] = &[20260927120000];

    let migration_table_exists: bool =
        sqlx::query_scalar("select to_regclass('public._sqlx_migrations') is not null")
            .fetch_one(pool)
            .await?;
    let applied_versions = if migration_table_exists {
        sqlx::query_scalar::<_, i64>(
            "select version from public._sqlx_migrations where success order by version",
        )
        .fetch_all(pool)
        .await?
    } else {
        Vec::new()
    };

    let mut migrator = sqlx::migrate!("./migrations");
    let resolved_versions = migrator
        .iter()
        .map(|migration| migration.version)
        .collect::<HashSet<_>>();
    let missing_versions = applied_versions
        .into_iter()
        .filter(|version| !resolved_versions.contains(version))
        .collect::<Vec<_>>();
    let unexpected_missing = missing_versions
        .iter()
        .filter(|version| !KNOWN_UNRESOLVED_PRODUCTION_MIGRATIONS.contains(version))
        .copied()
        .collect::<Vec<_>>();
    anyhow::ensure!(
        unexpected_missing.is_empty(),
        "applied SQLx migrations are missing from the embedded source: {unexpected_missing:?}"
    );
    if !missing_versions.is_empty() {
        tracing::warn!(
            migrations = ?missing_versions,
            "skipping only explicitly allowlisted applied migrations with unavailable source"
        );
    }
    migrator.set_ignore_missing(!missing_versions.is_empty());
    migrator.run(pool).await?;
    Ok(())
}
