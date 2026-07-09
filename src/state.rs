use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::Semaphore;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub profile_refresh_slots: Arc<Semaphore>,
}

impl AppState {
    pub fn new(pool: PgPool, config: Config) -> Self {
        let profile_refresh_concurrency = config.profile_refresh_concurrency;
        Self {
            pool,
            config,
            profile_refresh_slots: Arc::new(Semaphore::new(profile_refresh_concurrency)),
        }
    }
}
