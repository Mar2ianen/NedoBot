use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use sqlx::PgPool;
use teloxide::prelude::*;
use tokio::sync::mpsc;

use crate::{
    config::Config,
    db::telegram::{mark_user_profile_refresh_error, user_profile_needs_refresh},
    features::user_profiles::service::refresh_profile,
};

#[cfg(feature = "moderation")]
use crate::features::new_user_analysis::enqueue_new_user_audit_for_profile_refresh;

const PROFILE_REFRESH_QUEUE_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileRefreshEnqueueResult {
    Queued,
    Coalesced,
    Full,
    Closed,
}

#[derive(Clone)]
pub struct ProfileRefreshQueue {
    sender: mpsc::Sender<ProfileRefreshJob>,
    queued_jobs: Arc<Mutex<HashSet<(i64, i64)>>>,
}

#[derive(Clone, Copy)]
struct ProfileRefreshJob {
    chat_id: i64,
    user_id: i64,
}

impl ProfileRefreshQueue {
    fn new(
        capacity: usize,
    ) -> (
        Self,
        Arc<tokio::sync::Mutex<mpsc::Receiver<ProfileRefreshJob>>>,
    ) {
        let (sender, receiver) = mpsc::channel(capacity);
        let queue = Self {
            sender,
            queued_jobs: Arc::new(Mutex::new(HashSet::new())),
        };
        (queue, Arc::new(tokio::sync::Mutex::new(receiver)))
    }

    pub fn try_enqueue(&self, chat_id: i64, user_id: i64) -> ProfileRefreshEnqueueResult {
        let mut queued_jobs = self
            .queued_jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = (chat_id, user_id);
        if !queued_jobs.insert(key) {
            return ProfileRefreshEnqueueResult::Coalesced;
        }

        let job = ProfileRefreshJob { chat_id, user_id };
        match self.sender.try_send(job) {
            Ok(()) => ProfileRefreshEnqueueResult::Queued,
            Err(mpsc::error::TrySendError::Full(job)) => {
                queued_jobs.remove(&(job.chat_id, job.user_id));
                ProfileRefreshEnqueueResult::Full
            }
            Err(mpsc::error::TrySendError::Closed(job)) => {
                queued_jobs.remove(&(job.chat_id, job.user_id));
                ProfileRefreshEnqueueResult::Closed
            }
        }
    }

    fn mark_completed(&self, job: ProfileRefreshJob) {
        self.queued_jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&(job.chat_id, job.user_id));
    }
}

pub fn spawn_profile_refresh_workers(
    bot: Bot,
    pool: PgPool,
    config: Config,
) -> ProfileRefreshQueue {
    let (queue, receiver) = ProfileRefreshQueue::new(PROFILE_REFRESH_QUEUE_CAPACITY);

    for worker_index in 0..config.profile_refresh_concurrency {
        let bot = bot.clone();
        let pool = pool.clone();
        let config = config.clone();
        let queue = queue.clone();
        let receiver = receiver.clone();
        tokio::spawn(async move {
            while let Some(job) = receiver.lock().await.recv().await {
                process_profile_refresh_job(&bot, &pool, &config, job).await;
                queue.mark_completed(job);
            }
            tracing::warn!(worker_index, "profile refresh queue worker stopped");
        });
    }

    queue
}

async fn process_profile_refresh_job(
    bot: &Bot,
    pool: &PgPool,
    config: &Config,
    job: ProfileRefreshJob,
) {
    let should_refresh = match user_profile_needs_refresh(pool, job.user_id).await {
        Ok(should_refresh) => should_refresh,
        Err(err) => {
            tracing::warn!(%err, user_id = job.user_id, "failed to check user profile refresh state");
            false
        }
    };

    if should_refresh && let Err(err) = refresh_profile(bot, pool, job.user_id).await {
        let message = err.to_string();
        if let Err(save_err) = mark_user_profile_refresh_error(pool, job.user_id, &message).await {
            tracing::warn!(%save_err, user_id = job.user_id, "failed to save profile refresh error");
        }
        tracing::warn!(%err, user_id = job.user_id, "failed to refresh message author profile");
    }

    // Profile freshness is global, while the audit is keyed by (chat, user).
    // A fresh profile must not suppress the first audit in another community chat.
    process_profile_audit(pool, config, job).await;
}

async fn process_profile_audit(pool: &PgPool, config: &Config, job: ProfileRefreshJob) {
    #[cfg(not(feature = "moderation"))]
    let _ = (pool, config, job);

    #[cfg(feature = "moderation")]
    if config.new_user_audit_enabled
        && config.chat_allows(job.chat_id, |chat| chat.moderation)
        && let Err(err) =
            enqueue_new_user_audit_for_profile_refresh(pool, config, job.chat_id, job.user_id).await
    {
        tracing::warn!(%err, user_id = job.user_id, "failed to save unified new user audit baseline and job");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn queue_coalesces_user_events_and_releases_them_after_completion() {
        let (queue, receiver) = ProfileRefreshQueue::new(1);

        assert_eq!(
            queue.try_enqueue(-1001, 42),
            ProfileRefreshEnqueueResult::Queued
        );
        assert_eq!(
            queue.try_enqueue(-1001, 42),
            ProfileRefreshEnqueueResult::Coalesced
        );
        assert_eq!(
            queue.try_enqueue(-1002, 42),
            ProfileRefreshEnqueueResult::Full
        );
        assert_eq!(
            queue.try_enqueue(-1001, 43),
            ProfileRefreshEnqueueResult::Full
        );

        let job = receiver.lock().await.recv().await.unwrap();
        assert_eq!(job.user_id, 42);
        queue.mark_completed(job);

        assert_eq!(
            queue.try_enqueue(-1001, 42),
            ProfileRefreshEnqueueResult::Queued
        );
    }
}
