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
    features::{
        avatar_analysis::service::enqueue_current_avatar_analysis,
        first_message_spam::enqueue_first_message_spam_analysis,
        new_user_analysis::analyze_new_user_profile,
        spam_review::{create_review, send_review},
        user_profiles::service::refresh_profile,
    },
};

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
    queued_user_ids: Arc<Mutex<HashSet<i64>>>,
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
            queued_user_ids: Arc::new(Mutex::new(HashSet::new())),
        };
        (queue, Arc::new(tokio::sync::Mutex::new(receiver)))
    }

    pub fn try_enqueue(&self, chat_id: i64, user_id: i64) -> ProfileRefreshEnqueueResult {
        let mut queued_user_ids = self
            .queued_user_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !queued_user_ids.insert(user_id) {
            return ProfileRefreshEnqueueResult::Coalesced;
        }

        let job = ProfileRefreshJob { chat_id, user_id };
        match self.sender.try_send(job) {
            Ok(()) => ProfileRefreshEnqueueResult::Queued,
            Err(mpsc::error::TrySendError::Full(job)) => {
                queued_user_ids.remove(&job.user_id);
                ProfileRefreshEnqueueResult::Full
            }
            Err(mpsc::error::TrySendError::Closed(job)) => {
                queued_user_ids.remove(&job.user_id);
                ProfileRefreshEnqueueResult::Closed
            }
        }
    }

    fn mark_completed(&self, user_id: i64) {
        self.queued_user_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&user_id);
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
                queue.mark_completed(job.user_id);
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
    match user_profile_needs_refresh(pool, job.user_id).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(err) => {
            tracing::warn!(%err, user_id = job.user_id, "failed to check user profile refresh state");
            return;
        }
    }

    match refresh_profile(bot, pool, job.user_id).await {
        Ok(()) => {
            process_refreshed_profile(bot, pool, config, job).await;
        }
        Err(err) => {
            let message = err.to_string();
            if let Err(save_err) =
                mark_user_profile_refresh_error(pool, job.user_id, &message).await
            {
                tracing::warn!(%save_err, user_id = job.user_id, "failed to save profile refresh error");
            }
            tracing::warn!(%err, user_id = job.user_id, "failed to refresh message author profile");
        }
    }
}

async fn process_refreshed_profile(
    bot: &Bot,
    pool: &PgPool,
    config: &Config,
    job: ProfileRefreshJob,
) {
    if let Err(err) = analyze_new_user_profile(pool, job.chat_id, job.user_id).await {
        tracing::warn!(%err, user_id = job.user_id, "failed to analyze new user profile");
    } else {
        if let Err(err) =
            enqueue_first_message_spam_analysis(pool, config, job.chat_id, job.user_id).await
        {
            tracing::warn!(%err, user_id = job.user_id, "failed to enqueue first-message spam analysis");
        }
        match create_review(pool, job.chat_id, job.user_id).await {
            Ok(Some(review)) => {
                if let Err(err) = send_review(bot, &review).await {
                    tracing::warn!(%err, user_id = job.user_id, "failed to send spam review");
                }
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(%err, user_id = job.user_id, "failed to create spam review"),
        }
    }

    if config.avatar_classifier_enabled
        && let Err(err) = enqueue_current_avatar_analysis(pool, job.user_id).await
    {
        tracing::warn!(%err, user_id = job.user_id, "failed to enqueue avatar analysis");
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
            queue.try_enqueue(-1001, 43),
            ProfileRefreshEnqueueResult::Full
        );

        let job = receiver.lock().await.recv().await.unwrap();
        assert_eq!(job.user_id, 42);
        queue.mark_completed(job.user_id);

        assert_eq!(
            queue.try_enqueue(-1001, 42),
            ProfileRefreshEnqueueResult::Queued
        );
    }
}
