use sqlx::PgPool;
use std::sync::Arc;
use teloxide::drafter::InProcessRateLimiter;
use tokio::sync::Semaphore;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub ask_slots: Arc<Semaphore>,
    pub drafter_limiter: InProcessRateLimiter,
}

impl AppState {
    pub fn new(pool: PgPool, config: Config) -> Self {
        let ask_concurrency = config.ask_max_concurrency;
        Self {
            pool,
            config,
            ask_slots: Arc::new(Semaphore::new(ask_concurrency)),
            drafter_limiter: InProcessRateLimiter::default(),
        }
    }
}
