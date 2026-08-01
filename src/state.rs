use sqlx::PgPool;
use std::sync::Arc;
use teloxide::drafter::InProcessRateLimiter;
use teloxide::utils::time::{LlmMarkdownFormatter, MainMarkdownFormatter, TimeContext};
use tokio::sync::Semaphore;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub llm_formatter: Arc<LlmMarkdownFormatter>,
    pub main_formatter: Arc<MainMarkdownFormatter>,
    pub ask_slots: Arc<Semaphore>,
    pub drafter_limiter: InProcessRateLimiter,
}

impl AppState {
    pub fn new(pool: PgPool, config: Config) -> Self {
        let ask_concurrency = config.ask_max_concurrency;
        let time_context = TimeContext::from_name(&config.render_timezone)
            .expect("render_timezone must pass Config::validate_runtime_secrets");
        let time_context = Arc::new(time_context);
        Self {
            pool,
            config,
            llm_formatter: Arc::new(LlmMarkdownFormatter::new((*time_context).clone())),
            main_formatter: Arc::new(MainMarkdownFormatter::new((*time_context).clone())),
            ask_slots: Arc::new(Semaphore::new(ask_concurrency)),
            drafter_limiter: InProcessRateLimiter::default(),
        }
    }
}
