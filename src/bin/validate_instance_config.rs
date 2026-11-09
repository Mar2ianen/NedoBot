//! Проверяет production-профиль и наличие секретов без внешних запросов и записи в БД.

use tg_ai_bot_teloxide::config::Config;

fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    config.validate_runtime_secrets()?;
    println!("instance configuration is valid");
    Ok(())
}
