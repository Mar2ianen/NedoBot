use sqlx::PgPool;
use std::sync::Arc;
use teloxide::drafter::InProcessRateLimiter;
use teloxide::utils::{rich_text::LlmMarkdownFormatter, time::TimeContext};
use tokio::sync::Semaphore;

use crate::config::Config;
use crate::features::ask::metrics::AskDeliveryMetrics;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub render_time: Arc<TimeContext>,
    pub llm_formatter: Arc<LlmMarkdownFormatter>,
    pub ask_slots: Arc<Semaphore>,
    pub drafter_limiter: InProcessRateLimiter,
    pub ask_delivery_metrics: Arc<AskDeliveryMetrics>,
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
            render_time: Arc::clone(&time_context),
            llm_formatter: Arc::new(LlmMarkdownFormatter::new()),
            ask_slots: Arc::new(Semaphore::new(ask_concurrency)),
            drafter_limiter: InProcessRateLimiter::default(),
            ask_delivery_metrics: Arc::new(AskDeliveryMetrics::default()),
        }
    }
}
