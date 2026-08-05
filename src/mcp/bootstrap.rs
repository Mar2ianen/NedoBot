//! Shared safe construction for RMCP chat read-model transports.

use std::{env, sync::Arc};

use anyhow::{Context, bail};
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::{
    features::chat_read_api::{
        ChatReadApi,
        catalog::PublicCatalog,
        types::{
            CHAT_EMBEDDING_MODEL_ENV, CHAT_EMBEDDING_TIMEOUT_ENV, CHAT_EMBEDDING_URL_ENV,
            SemanticSearchConfig,
        },
    },
    mcp::server::ChatMcpServer,
};

pub const DATABASE_URL_ENV: &str = "ASK_DATABASE_URL";
pub const MANIFEST_PATH_ENV: &str = "MCP_MANIFEST";

/// Required runtime configuration for the standalone RMCP stdio server.
pub struct RmcpStdioConfig {
    database_url: String,
    manifest_path: String,
    semantic_search: Option<SemanticSearchConfig>,
}

impl RmcpStdioConfig {
    /// Reads every child-process setting explicitly; this binary never loads `.env`.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::new(
            required_env(DATABASE_URL_ENV)?,
            required_env(MANIFEST_PATH_ENV)?,
        )
    }

    /// Creates shared RMCP bootstrap settings after validating both required values.
    pub fn new(database_url: String, manifest_path: String) -> anyhow::Result<Self> {
        Ok(Self {
            database_url: required_value(DATABASE_URL_ENV, database_url)?,
            manifest_path: required_value(MANIFEST_PATH_ENV, manifest_path)?,
            semantic_search: semantic_search_from_env()?,
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

fn semantic_search_from_env() -> anyhow::Result<Option<SemanticSearchConfig>> {
    let embedding_url = env::var(CHAT_EMBEDDING_URL_ENV).ok();
    let embedding_model = env::var(CHAT_EMBEDDING_MODEL_ENV).ok();
    let timeout = env::var(CHAT_EMBEDDING_TIMEOUT_ENV).ok();
    if embedding_url.is_none() && embedding_model.is_none() && timeout.is_none() {
        return Ok(None);
    }

    let embedding_url = required_value(
        CHAT_EMBEDDING_URL_ENV,
        embedding_url.ok_or_else(|| anyhow::anyhow!("{CHAT_EMBEDDING_URL_ENV} is required"))?,
    )?;
    let parsed_url = reqwest::Url::parse(&embedding_url)
        .with_context(|| format!("{CHAT_EMBEDDING_URL_ENV} must be a valid URL"))?;
    anyhow::ensure!(
        matches!(parsed_url.scheme(), "http" | "https"),
        "{CHAT_EMBEDDING_URL_ENV} must use http or https"
    );
    let embedding_model = required_value(
        CHAT_EMBEDDING_MODEL_ENV,
        embedding_model.ok_or_else(|| anyhow::anyhow!("{CHAT_EMBEDDING_MODEL_ENV} is required"))?,
    )?;
    let timeout_sec = timeout
        .ok_or_else(|| anyhow::anyhow!("{CHAT_EMBEDDING_TIMEOUT_ENV} is required"))?
        .parse::<u64>()
        .with_context(|| format!("{CHAT_EMBEDDING_TIMEOUT_ENV} must be an integer"))?;
    anyhow::ensure!(
        timeout_sec > 0,
        "{CHAT_EMBEDDING_TIMEOUT_ENV} must be greater than zero"
    );

    Ok(Some(SemanticSearchConfig {
        embedding_url,
        embedding_model,
        timeout_sec,
    }))
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
    let api = ChatReadApi::new_with_semantic_search(
        pool,
        catalog.scope(),
        catalog,
        config.semantic_search,
    )?;
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
