//! Legacy stdio entry point for the shared chat read-model.
//!
//! The protocol adapter is intentionally thin; SQL and read policy belong to
//! `features::chat_read_api` and are shared with other transports.
pub async fn run_stdio_server() -> anyhow::Result<()> {
    crate::features::chat_read_api::ChatReadApi::run_internal_stdio().await
}
