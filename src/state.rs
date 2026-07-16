use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::Semaphore;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub profile_refresh_slots: Arc<Semaphore>,
    pub avatar_classifier_slots: Arc<Semaphore>,
    pub ask_slots: Arc<Semaphore>,
}

impl AppState {
    pub fn new(pool: PgPool, config: Config) -> Self {
        let profile_refresh_concurrency = config.profile_refresh_concurrency;
        let avatar_classifier_concurrency = config.avatar_classifier_concurrency;
        let ask_concurrency = config.ask_max_concurrency;
        Self {
            pool,
            config,
            profile_refresh_slots: Arc::new(Semaphore::new(profile_refresh_concurrency)),
            avatar_classifier_slots: Arc::new(Semaphore::new(avatar_classifier_concurrency)),
            ask_slots: Arc::new(Semaphore::new(ask_concurrency)),
        }
    }
}
