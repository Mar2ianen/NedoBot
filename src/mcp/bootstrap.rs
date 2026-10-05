//! Shared safe construction for RMCP chat read-model transports.

use std::{env, sync::Arc};

use anyhow::{Context, bail};
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::{
    features::{
        chat_read_api::{ChatReadApi, catalog::PublicCatalog},
        search::youtube::YoutubeSubtitlesConfig,
    },
    mcp::server::ChatMcpServer,
};

pub const DATABASE_URL_ENV: &str = "ASK_DATABASE_URL";
pub const MANIFEST_PATH_ENV: &str = "MCP_MANIFEST";
const YOUTUBE_SUBTITLES_ENABLED_ENV: &str = "MCP_YOUTUBE_SUBTITLES_ENABLED";
const YOUTUBE_SUBTITLES_COMMAND_ENV: &str = "MCP_YOUTUBE_SUBTITLES_COMMAND";
const YOUTUBE_SUBTITLES_LANGUAGES_ENV: &str = "MCP_YOUTUBE_SUBTITLES_LANGUAGES";
const YOUTUBE_SUBTITLES_TIMEOUT_ENV: &str = "MCP_YOUTUBE_SUBTITLES_TIMEOUT_SEC";
const YOUTUBE_SUBTITLES_MAX_CHARS_ENV: &str = "MCP_YOUTUBE_SUBTITLES_MAX_CHARS";
const YOUTUBE_SUBTITLES_MAX_VIDEOS_ENV: &str = "MCP_YOUTUBE_SUBTITLES_MAX_VIDEOS";

/// Required runtime configuration for the standalone RMCP stdio server.
pub struct RmcpStdioConfig {
    database_url: String,
    manifest_path: String,
    youtube_subtitles: YoutubeSubtitlesConfig,
}

impl RmcpStdioConfig {
    /// Reads every child-process setting explicitly; this binary never loads `.env`.
    pub fn from_env() -> anyhow::Result<Self> {
        let mut config = Self::new(
            required_env(DATABASE_URL_ENV)?,
            required_env(MANIFEST_PATH_ENV)?,
        )?;
        config.youtube_subtitles = youtube_subtitles_from_env()?;
        Ok(config)
    }

    /// Creates shared RMCP bootstrap settings after validating both required values.
    pub fn new(database_url: String, manifest_path: String) -> anyhow::Result<Self> {
        Ok(Self {
            database_url: required_value(DATABASE_URL_ENV, database_url)?,
            manifest_path: required_value(MANIFEST_PATH_ENV, manifest_path)?,
            youtube_subtitles: YoutubeSubtitlesConfig::disabled(),
        })
    }

    pub(crate) fn with_youtube_subtitles(mut self, config: YoutubeSubtitlesConfig) -> Self {
        self.youtube_subtitles = config;
        self
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

pub(crate) fn youtube_subtitles_from_env() -> anyhow::Result<YoutubeSubtitlesConfig> {
    let config = YoutubeSubtitlesConfig {
        enabled: parse_bool_env(YOUTUBE_SUBTITLES_ENABLED_ENV, false)?,
        command: env::var(YOUTUBE_SUBTITLES_COMMAND_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .or_else(|| Some("yt-dlp".to_string())),
        languages: env::var(YOUTUBE_SUBTITLES_LANGUAGES_ENV)
            .unwrap_or_else(|_| "ru,ru.*,en,en.*".to_string())
            .split(',')
            .map(|language| language.trim().to_string())
            .collect(),
        timeout_sec: parse_env(YOUTUBE_SUBTITLES_TIMEOUT_ENV, 15, "a positive integer")?,
        max_chars: parse_env(
            YOUTUBE_SUBTITLES_MAX_CHARS_ENV,
            12_000,
            "a positive integer",
        )?,
        max_videos: parse_env(YOUTUBE_SUBTITLES_MAX_VIDEOS_ENV, 2, "a positive integer")?,
    };
    config
        .validate()
        .context("MCP YouTube subtitles configuration is invalid")?;
    Ok(config)
}

fn parse_bool_env(name: &str, default: bool) -> anyhow::Result<bool> {
    let Some(value) = env::var(name).ok() else {
        return Ok(default);
    };
    match value.trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => bail!("{name} must be true, false, 1, or 0"),
    }
}

fn parse_env<T>(name: &str, default: T, expected: &str) -> anyhow::Result<T>
where
    T: std::str::FromStr,
{
    let Some(value) = env::var(name).ok() else {
        return Ok(default);
    };
    value
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("{name} must be {expected}"))
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
    Ok(ChatMcpServer::new(Arc::new(api)).with_youtube_subtitles(config.youtube_subtitles))
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
