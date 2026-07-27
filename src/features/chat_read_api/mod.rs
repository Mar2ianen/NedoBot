//! Транспорт-независимый read-model для внутреннего и публичного MCP.
//!
//! Этот модуль содержит все SQL-запросы и allowlisted-каталог. Транспортные
//! адаптеры только принимают запросы и делегируют выполнение сюда.

pub mod catalog;
pub mod internal;
pub mod policy;
pub mod service;
pub mod types;

/// Единая точка входа для legacy MCP-транспортов.
pub struct ChatReadApi;

impl ChatReadApi {
    pub async fn run_internal_stdio() -> anyhow::Result<()> {
        internal::run_stdio_server().await
    }

    pub async fn run_public_http() -> anyhow::Result<()> {
        catalog::run_public_http().await
    }
}
