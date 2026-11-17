//! Shared lifecycle for long-lived application workers.
//!
//! Domain workers retain ownership of their claims and durable transitions. This
//! module only tracks their tasks, signals shutdown, bounds draining, and
//! centralizes the idle/error polling cadence.

use std::{collections::HashMap, future::Future, time::Duration};

use tokio::{
    sync::watch,
    task::{Id, JoinError, JoinSet},
    time::sleep,
};

use super::policy::WorkerPollPolicy;

pub const WORKER_DRAIN_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerId {
    AskMemoryExpiry,
    SpamReputationSync,
    LinkedChatRefresh,
    UrlFeedRefresh,
    QrEvidence,
    DecisionAudit,
    ProfileRefresh,
    SpammerAvatarEmbedding,
    NewUserAudit,
    RiskCaptchaLifecycle,
    SpamReviewDelivery,
    ReportDelivery,
    FirstComment,
    PostHistory,
    ChatRetrievalEmbedding,
    VoiceTranscription,
    Unknown,
}

impl WorkerId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AskMemoryExpiry => "ask_memory_expiry",
            Self::SpamReputationSync => "spam_reputation_sync",
            Self::LinkedChatRefresh => "linked_chat_refresh",
            Self::UrlFeedRefresh => "url_feed_refresh",
            Self::QrEvidence => "qr_evidence",
            Self::DecisionAudit => "decision_audit",
            Self::ProfileRefresh => "profile_refresh",
            Self::SpammerAvatarEmbedding => "spammer_avatar_embedding",
            Self::NewUserAudit => "new_user_audit",
            Self::RiskCaptchaLifecycle => "risk_captcha_lifecycle",
            Self::SpamReviewDelivery => "spam_review_delivery",
            Self::ReportDelivery => "report_delivery",
            Self::FirstComment => "first_comment",
            Self::PostHistory => "post_history",
            Self::ChatRetrievalEmbedding => "chat_retrieval_embedding",
            Self::VoiceTranscription => "voice_transcription",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WorkerSpec {
    pub id: WorkerId,
    pub poll: WorkerPollPolicy,
}

impl WorkerSpec {
    pub const fn new(id: WorkerId, poll: WorkerPollPolicy) -> Self {
        Self { id, poll }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    Processed,
    Idle,
    Failed(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerTaskExit {
    Completed,
    Panicked,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerFailure {
    pub worker: WorkerId,
    pub exit: WorkerTaskExit,
}

impl std::fmt::Display for WorkerFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "background worker {:?} exited unexpectedly: {:?}",
            self.worker, self.exit
        )
    }
}

impl std::error::Error for WorkerFailure {}

pub struct WorkerSupervisor {
    shutdown: watch::Sender<bool>,
    tasks: JoinSet<WorkerId>,
    worker_ids: HashMap<Id, WorkerId>,
}

impl Default for WorkerSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerSupervisor {
    pub fn new() -> Self {
        let (shutdown, _) = watch::channel(false);
        Self {
            shutdown,
            tasks: JoinSet::new(),
            worker_ids: HashMap::new(),
        }
    }

    pub fn spawn<F, Fut>(&mut self, id: WorkerId, worker: F)
    where
        F: FnOnce(watch::Receiver<bool>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let shutdown = self.shutdown.subscribe();
        let handle = self.tasks.spawn(async move {
            worker(shutdown).await;
            id
        });
        self.worker_ids.insert(handle.id(), id);
    }

    /// Runs the foreground service while observing unexpected worker exits.
    /// A panic is reported by worker ID; durable jobs remain recoverable through
    /// their existing database leases on restart.
    pub async fn supervise_until<F>(&mut self, foreground: F) -> Result<(), WorkerFailure>
    where
        F: Future<Output = ()>,
    {
        tokio::pin!(foreground);
        loop {
            tokio::select! {
                _ = &mut foreground => {
                    self.shutdown_and_drain().await;
                    return Ok(());
                },
                joined = self.tasks.join_next_with_id(), if !self.tasks.is_empty() => {
                    if let Some(joined) = joined {
                        let (worker, exit) = self.record_exit(joined);
                        self.shutdown_and_drain().await;
                        return Err(WorkerFailure { worker, exit });
                    }
                }
            }
        }
    }

    pub async fn shutdown_and_drain(&mut self) {
        self.shutdown_and_drain_with_timeout(WORKER_DRAIN_TIMEOUT)
            .await;
    }

    async fn shutdown_and_drain_with_timeout(&mut self, drain_timeout: Duration) {
        self.shutdown.send_replace(true);
        let drain = async {
            while let Some(joined) = self.tasks.join_next_with_id().await {
                self.record_exit(joined);
            }
        };
        if tokio::time::timeout(drain_timeout, drain).await.is_err() {
            let abandoned = self.tasks.len();
            self.tasks.abort_all();
            while let Some(joined) = self.tasks.join_next_with_id().await {
                self.record_exit(joined);
            }
            tracing::warn!(
                abandoned,
                drain_timeout_ms = drain_timeout.as_millis(),
                "aborted background workers after bounded shutdown drain"
            );
        }
    }

    #[cfg(test)]
    async fn reap_finished(&mut self) -> Vec<(WorkerId, WorkerTaskExit)> {
        let mut exits = Vec::new();
        while let Some(joined) = self.tasks.try_join_next_with_id() {
            exits.push(self.classify_join(joined));
        }
        exits
    }

    #[cfg(test)]
    fn abort_all(&mut self) {
        self.tasks.abort_all();
    }

    fn classify_join(
        &mut self,
        joined: Result<(Id, WorkerId), JoinError>,
    ) -> (WorkerId, WorkerTaskExit) {
        match joined {
            Ok((id, worker)) => {
                self.worker_ids.remove(&id);
                (worker, WorkerTaskExit::Completed)
            }
            Err(error) => {
                let worker = self.worker_ids.remove(&error.id()).unwrap_or_else(|| {
                    tracing::error!(task_id = ?error.id(), "worker task exited without a registered worker ID");
                    WorkerId::Unknown
                });
                let exit = if error.is_panic() {
                    WorkerTaskExit::Panicked
                } else {
                    WorkerTaskExit::Aborted
                };
                (worker, exit)
            }
        }
    }

    fn record_exit(
        &mut self,
        joined: Result<(Id, WorkerId), JoinError>,
    ) -> (WorkerId, WorkerTaskExit) {
        let shutting_down = *self.shutdown.borrow();
        let (worker, exit) = self.classify_join(joined);
        match exit {
            WorkerTaskExit::Completed if shutting_down => {
                tracing::info!(worker = worker.as_str(), "background worker stopped");
            }
            WorkerTaskExit::Completed => {
                tracing::error!(
                    worker = worker.as_str(),
                    "background worker exited unexpectedly"
                );
            }
            WorkerTaskExit::Panicked => {
                tracing::error!(
                    worker = worker.as_str(),
                    "background worker panicked; its lease will recover on restart"
                );
            }
            WorkerTaskExit::Aborted => {
                tracing::warn!(
                    worker = worker.as_str(),
                    "background worker task was aborted"
                );
            }
        }
        (worker, exit)
    }
}

pub async fn run_poll_worker<F, Fut>(
    spec: WorkerSpec,
    mut shutdown: watch::Receiver<bool>,
    mut poll: F,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = PollOutcome>,
{
    loop {
        if *shutdown.borrow() {
            return;
        }

        let delay_seconds = match poll().await {
            PollOutcome::Processed => continue,
            PollOutcome::Idle => spec.poll.idle_seconds(),
            PollOutcome::Failed(error_kind) => {
                tracing::warn!(
                    worker = spec.id.as_str(),
                    error_kind,
                    "background worker iteration failed"
                );
                spec.poll.error_seconds()
            }
        };

        tokio::select! {
            _ = sleep(Duration::from_secs(delay_seconds)) => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

pub async fn run_interval_worker<F, Fut>(
    id: WorkerId,
    mut shutdown: watch::Receiver<bool>,
    period: Duration,
    skip_missed_ticks: bool,
    delay_first_tick: bool,
    mut operation: F,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<&'static str>>,
{
    let mut interval = tokio::time::interval(period);
    if skip_missed_ticks {
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    }
    if delay_first_tick {
        tokio::select! {
            _ = interval.tick() => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
        }
    }
    loop {
        tokio::select! {
            _ = interval.tick() => {
                if *shutdown.borrow() {
                    return;
                }
                if let Some(error_kind) = operation().await {
                    tracing::warn!(worker = id.as_str(), error_kind, "periodic worker operation failed");
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

pub async fn run_fixed_delay_worker<F, Fut>(
    id: WorkerId,
    mut shutdown: watch::Receiver<bool>,
    delay: Duration,
    mut operation: F,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<&'static str>>,
{
    loop {
        if *shutdown.borrow() {
            return;
        }
        if let Some(error_kind) = operation().await {
            tracing::warn!(
                worker = id.as_str(),
                error_kind,
                "periodic worker operation failed"
            );
        }
        tokio::select! {
            _ = sleep(delay) => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::super::policy::WorkerPollPolicy;
    use super::run_interval_worker;
    use super::{
        PollOutcome, WorkerFailure, WorkerId, WorkerSpec, WorkerSupervisor, WorkerTaskExit,
        run_poll_worker,
    };

    #[tokio::test(start_paused = true)]
    async fn poll_worker_uses_idle_policy_and_stops_during_backoff() {
        let supervisor = WorkerSupervisor::new();
        let shutdown = supervisor.shutdown.subscribe();
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = calls.clone();
        let worker = tokio::spawn(async move {
            run_poll_worker(
                WorkerSpec::new(WorkerId::QrEvidence, WorkerPollPolicy::new(5, 9)),
                shutdown.clone(),
                move || {
                    worker_calls.fetch_add(1, Ordering::SeqCst);
                    async { PollOutcome::Idle }
                },
            )
            .await;
        });
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(std::time::Duration::from_secs(4)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        supervisor.shutdown.send_replace(true);
        worker.await.expect("worker should stop after cancellation");
    }

    #[tokio::test(start_paused = true)]
    async fn delayed_interval_worker_stops_before_first_operation() {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = calls.clone();
        let worker = tokio::spawn(async move {
            run_interval_worker(
                WorkerId::LinkedChatRefresh,
                shutdown_rx,
                std::time::Duration::from_secs(60),
                true,
                true,
                move || {
                    worker_calls.fetch_add(1, Ordering::SeqCst);
                    async { None }
                },
            )
            .await;
        });
        tokio::task::yield_now().await;
        shutdown_tx.send_replace(true);
        worker.await.expect("worker should stop on cancellation");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn shutdown_waits_for_current_poll_to_finish() {
        let mut supervisor = WorkerSupervisor::new();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let completed = Arc::new(AtomicUsize::new(0));
        let worker_completed = completed.clone();
        supervisor.spawn(WorkerId::DecisionAudit, move |shutdown| async move {
            let mut first = Some(started_tx);
            let mut finish = Some(finish_rx);
            run_poll_worker(
                WorkerSpec::new(WorkerId::DecisionAudit, WorkerPollPolicy::new(1, 1)),
                shutdown,
                move || {
                    let started = first.take();
                    let finishing = finish.take();
                    let completed = worker_completed.clone();
                    async move {
                        if let Some(started) = started {
                            let _ = started.send(());
                            let _ = finishing.expect("first poll receiver").await;
                            completed.fetch_add(1, Ordering::SeqCst);
                            PollOutcome::Processed
                        } else {
                            PollOutcome::Idle
                        }
                    }
                },
            )
            .await;
        });
        started_rx.await.expect("first poll must start");
        supervisor.shutdown.send_replace(true);
        finish_tx
            .send(())
            .expect("in-flight poll must still be running");
        supervisor.shutdown_and_drain().await;
        assert_eq!(completed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn supervisor_reports_panics_and_aborts() {
        let mut supervisor = WorkerSupervisor::new();
        supervisor.spawn(WorkerId::NewUserAudit, |_| async {
            panic!("synthetic worker panic");
        });
        tokio::task::yield_now().await;
        assert_eq!(
            supervisor.reap_finished().await,
            vec![(WorkerId::NewUserAudit, WorkerTaskExit::Panicked)]
        );

        supervisor.spawn(WorkerId::QrEvidence, |_| async {
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        supervisor.abort_all();
        let aborted = supervisor
            .tasks
            .join_next_with_id()
            .await
            .expect("aborted worker should be joined");
        assert_eq!(
            supervisor.classify_join(aborted),
            (WorkerId::QrEvidence, WorkerTaskExit::Aborted)
        );
    }

    #[tokio::test]
    async fn unexpected_worker_return_is_fatal_after_other_workers_drain() {
        let mut supervisor = WorkerSupervisor::new();
        let (stopped_tx, stopped_rx) = tokio::sync::oneshot::channel();
        supervisor.spawn(WorkerId::NewUserAudit, |_| async {});
        supervisor.spawn(WorkerId::QrEvidence, move |mut shutdown| async move {
            let _ = shutdown.changed().await;
            let _ = stopped_tx.send(());
        });

        let failure = supervisor
            .supervise_until(std::future::pending())
            .await
            .expect_err("a worker returning before shutdown must fail the service");

        assert_eq!(
            failure,
            WorkerFailure {
                worker: WorkerId::NewUserAudit,
                exit: WorkerTaskExit::Completed,
            }
        );
        stopped_rx
            .await
            .expect("remaining workers must drain before the fatal result is returned");
    }

    #[tokio::test]
    async fn worker_panic_is_fatal_after_other_workers_drain() {
        let mut supervisor = WorkerSupervisor::new();
        let (stopped_tx, stopped_rx) = tokio::sync::oneshot::channel();
        supervisor.spawn(WorkerId::DecisionAudit, |_| async {
            panic!("synthetic worker panic");
        });
        supervisor.spawn(WorkerId::QrEvidence, move |mut shutdown| async move {
            let _ = shutdown.changed().await;
            let _ = stopped_tx.send(());
        });

        let failure = supervisor
            .supervise_until(std::future::pending())
            .await
            .expect_err("a worker panic must fail the service");

        assert_eq!(
            failure,
            WorkerFailure {
                worker: WorkerId::DecisionAudit,
                exit: WorkerTaskExit::Panicked,
            }
        );
        stopped_rx
            .await
            .expect("remaining workers must drain before the fatal result is returned");
    }

    #[tokio::test]
    async fn foreground_shutdown_drains_workers_without_failing() {
        let mut supervisor = WorkerSupervisor::new();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (stopped_tx, stopped_rx) = tokio::sync::oneshot::channel();
        supervisor.spawn(WorkerId::QrEvidence, move |mut shutdown| async move {
            let _ = started_tx.send(());
            let _ = shutdown.changed().await;
            let _ = stopped_tx.send(());
        });

        let result = supervisor
            .supervise_until(async {
                started_rx
                    .await
                    .expect("worker must start before foreground shutdown");
            })
            .await;

        assert!(result.is_ok(), "normal dispatcher shutdown is successful");
        stopped_rx
            .await
            .expect("normal shutdown must drain the worker");
    }

    #[tokio::test]
    async fn shutdown_aborts_worker_after_the_drain_deadline() {
        let mut supervisor = WorkerSupervisor::new();
        supervisor.spawn(WorkerId::QrEvidence, |_| async {
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;

        supervisor
            .shutdown_and_drain_with_timeout(std::time::Duration::from_millis(1))
            .await;

        assert!(
            supervisor.tasks.is_empty(),
            "timed-out tasks must be joined"
        );
        assert!(supervisor.worker_ids.is_empty());
    }
}
