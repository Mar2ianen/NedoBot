//! Shared safe construction for RMCP chat read-model transports.

use std::{env, sync::Arc};

use anyhow::{Context, bail};
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::{
    features::chat_read_api::{ChatReadApi, catalog::PublicCatalog},
    mcp::server::ChatMcpServer,
};

pub const DATABASE_URL_ENV: &str = "ASK_DATABASE_URL";
pub const MANIFEST_PATH_ENV: &str = "MCP_MANIFEST";

/// Required runtime configuration for the standalone RMCP stdio server.
pub struct RmcpStdioConfig {
    database_url: String,
    manifest_path: String,
}

impl RmcpStdioConfig {
    /// Reads every child-process setting explicitly; this binary never loads `.env`.
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            database_url: required_env(DATABASE_URL_ENV)?,
            manifest_path: required_env(MANIFEST_PATH_ENV)?,
        })
    }
}

fn required_env(name: &str) -> anyhow::Result<String> {
    let value = env::var(name).with_context(|| format!("{name} is required"))?;
    required_value(name, value)
}

fn required_value(name: &str, value: String) -> anyhow::Result<String> {
    if value.trim().is_empty() {
        bail!("{name} must not be empty");
    }
    Ok(value)
}

/// Builds a pool that rejects writes even when a tool implementation regresses.
pub async fn build_readonly_pool(database_url: &str) -> anyhow::Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("set default_transaction_read_only = on")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("set statement_timeout = '5s'")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("set lock_timeout = '1s'")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("set idle_in_transaction_session_timeout = '5s'")
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .context("MCP database connection failed")
}

/// Loads and validates the reviewed catalog before exposing any MCP tool.
pub async fn build_chat_mcp_server(config: RmcpStdioConfig) -> anyhow::Result<ChatMcpServer> {
    let catalog = PublicCatalog::load(&config.manifest_path)?;
    let pool = build_readonly_pool(&config.database_url).await?;
    let api = ChatReadApi::new(pool, catalog.scope(), catalog)?;
    api.validate().await?;
    Ok(ChatMcpServer::new(Arc::new(api)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_value_rejects_blank_value() {
        let error = required_value(MANIFEST_PATH_ENV, " \t".into())
            .expect_err("blank required variable must fail");
        assert_eq!(
            error.to_string(),
            format!("{MANIFEST_PATH_ENV} must not be empty")
        );
    }
}
