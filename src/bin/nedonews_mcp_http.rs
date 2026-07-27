//! Public HTTP entry point for the shared NedoNews chat read-model.
//! SQL, catalog policy, and tool execution live in `features::chat_read_api`.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tg_ai_bot_teloxide::features::chat_read_api::ChatReadApi::run_public_http().await
}
