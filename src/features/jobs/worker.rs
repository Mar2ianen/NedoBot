//! Shared lifecycle for long-lived application workers.
//!
//! Domain workers retain ownership of their claims and durable transitions. This
//! module only tracks their tasks, signals shutdown, bounds draining, and
//! centralizes the idle/error polling cadence.

use std::{collections::HashMap, future::Future, time::{Duration, Instant}};

use tokio::{
    sync::watch,
    task::{Id, JoinError, JoinSet},
    time::sleep,
};

use super::policy::WorkerPollPolicy;
use crate::features::health::{self, WorkerOutcome};

pub const WORKER_DRAIN_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerId {
    AskMemoryExpiry,
    SpamReputationSync,
    LinkedChatRefresh,
    UrlFeedRefresh,
    QrEvidence,
    QrShadowRefresh,
    DecisionAudit,
    ProfileRefresh,
    ProfileRefreshReconciler,
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
            Self::QrShadowRefresh => "qr_shadow_refresh",
            Self::DecisionAudit => "decision_audit",
            Self::ProfileRefresh => "profile_refresh",
            Self::ProfileRefreshReconciler => "profile_refresh_reconciler",
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
            "background worker {} exited unexpectedly ({:?}); process restart required",
            self.worker.as_str(),
            self.exit
        )
    }
}

impl std::error::Error for WorkerFailure {}

pub struct WorkerSupervisor {
    shutdown: watch::Sender<bool>,
    tasks: JoinSet<WorkerId>,
    worker_ids: HashMap<Id, WorkerId>,
    drain_timeout: Duration,
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
            drain_timeout: WORKER_DRAIN_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_drain_timeout(drain_timeout: Duration) -> Self {
        Self {
            drain_timeout,
            ..Self::new()
        }
    }

    pub fn spawn<F, Fut>(&mut self, id: WorkerId, worker: F)
    where
        F: FnOnce(watch::Receiver<bool>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        health::worker_registered(id.as_str());
        let shutdown = self.shutdown.subscribe();
        let handle = self.tasks.spawn(async move {
            worker(shutdown).await;
            id
        });
        self.worker_ids.insert(handle.id(), id);
    }

    /// Runs the foreground service while observing unexpected worker exits.
    /// A long-lived worker exiting unexpectedly is fatal: after draining sibling
    /// workers, return an error so the process supervisor can restart the service.
    pub async fn supervise_until<F>(&mut self, foreground: F) -> Result<(), WorkerFailure>
    where
        F: Future<Output = ()>,
    {
        tokio::pin!(foreground);
        let mut failure = None;
        loop {
            tokio::select! {
                _ = &mut foreground => break,
                joined = self.tasks.join_next_with_id(), if !self.tasks.is_empty() => {
                    if let Some(joined) = joined {
                        failure = self.record_exit(joined);
                        if failure.is_some() {
                            break;
                        }
                    }
                }
            }
        }
        self.shutdown_and_drain().await;
        failure.map_or(Ok(()), Err)
    }

    pub async fn shutdown_and_drain(&mut self) {
        self.shutdown.send_replace(true);
        let drain_timeout = self.drain_timeout;
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
                drain_timeout_ms = self.drain_timeout.as_millis() as u64,
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

    fn record_exit(&mut self, joined: Result<(Id, WorkerId), JoinError>) -> Option<WorkerFailure> {
        let shutting_down = *self.shutdown.borrow();
        let (worker, exit) = self.classify_join(joined);
        health::worker_exited(worker.as_str());
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
        (!shutting_down).then_some(WorkerFailure { worker, exit })
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

        let poll_started = Instant::now();
        let outcome = poll().await;
        let kind = match &outcome {
            PollOutcome::Processed => WorkerOutcome::Processed,
            PollOutcome::Idle => WorkerOutcome::Idle,
            PollOutcome::Failed(_) => WorkerOutcome::Failed,
        };
        health::worker_polled(spec.id.as_str(), kind, poll_started.elapsed());
        let delay_seconds = match outcome {
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
                let started = Instant::now();
                let error = operation().await;
                health::worker_polled(id.as_str(), if error.is_some() { WorkerOutcome::Failed } else { WorkerOutcome::Processed }, started.elapsed());
                if let Some(error_kind) = error {
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
        let started = Instant::now();
        let error = operation().await;
        health::worker_polled(id.as_str(), if error.is_some() { WorkerOutcome::Failed } else { WorkerOutcome::Processed }, started.elapsed());
        if let Some(error_kind) = error {
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
    use super::super::policy::WorkerPollPolicy;
    use super::run_interval_worker;
    use super::{
        PollOutcome, WorkerFailure, WorkerId, WorkerSpec, WorkerSupervisor, WorkerTaskExit,
        run_poll_worker,
    };
    use std::{
        io::{self, Write},
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    #[derive(Clone)]
    struct CaptureLogs(Arc<Mutex<Vec<u8>>>);

    impl Write for CaptureLogs {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("capture mutex")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureLogs {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn capture_logs(operation: impl FnOnce()) -> String {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(CaptureLogs(Arc::clone(&bytes)))
            .finish();
        tracing::subscriber::with_default(subscriber, operation);
        String::from_utf8(bytes.lock().expect("capture mutex").clone())
            .expect("tracing output is UTF-8")
    }

    async fn supervise_pending(supervisor: &mut WorkerSupervisor) -> Result<(), WorkerFailure> {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            supervisor.supervise_until(std::future::pending::<()>()),
        )
        .await
        .expect(
            "supervision must terminate after an unexpected worker exit instead of waiting forever",
        )
    }

    #[tokio::test]
    async fn record_exit_logs_graceful_and_unexpected_completion_differently() {
        let mut supervisor = WorkerSupervisor::new();
        supervisor.spawn(WorkerId::ProfileRefresh, |_| async {});
        tokio::task::yield_now().await;
        let joined = supervisor
            .tasks
            .join_next_with_id()
            .await
            .expect("completed worker must be joined");
        let mut fatal = None;
        let unexpected = capture_logs(|| fatal = supervisor.record_exit(joined));
        assert_eq!(
            fatal,
            Some(WorkerFailure {
                worker: WorkerId::ProfileRefresh,
                exit: WorkerTaskExit::Completed,
            })
        );
        assert!(unexpected.contains("exited unexpectedly"));
        assert!(!unexpected.contains("background worker stopped"));

        supervisor.shutdown.send_replace(true);
        supervisor.spawn(WorkerId::ProfileRefresh, |_| async {});
        tokio::task::yield_now().await;
        let joined = supervisor
            .tasks
            .join_next_with_id()
            .await
            .expect("stopped worker must be joined");
        let graceful = capture_logs(|| {
            assert_eq!(supervisor.record_exit(joined), None);
        });
        assert!(graceful.contains("background worker stopped"));
        assert!(!graceful.contains("exited unexpectedly"));
    }

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
    async fn unexpected_worker_panic_fails_foreground_supervision() {
        let mut supervisor = WorkerSupervisor::new();
        let sibling_stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sibling_stopped_by_worker = Arc::clone(&sibling_stopped);
        supervisor.spawn(WorkerId::ProfileRefresh, move |mut shutdown| async move {
            let _ = shutdown.changed().await;
            sibling_stopped_by_worker.store(true, Ordering::SeqCst);
        });
        supervisor.spawn(WorkerId::DecisionAudit, |_| async {
            panic!("synthetic critical worker panic");
        });

        let result = supervise_pending(&mut supervisor).await;

        assert_eq!(
            result.unwrap_err(),
            WorkerFailure {
                worker: WorkerId::DecisionAudit,
                exit: WorkerTaskExit::Panicked,
            }
        );
        assert!(sibling_stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn unexpected_worker_completion_fails_foreground_supervision() {
        let mut supervisor = WorkerSupervisor::new();
        let sibling_stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sibling_stopped_by_worker = Arc::clone(&sibling_stopped);
        supervisor.spawn(WorkerId::ProfileRefresh, move |mut shutdown| async move {
            let _ = shutdown.changed().await;
            sibling_stopped_by_worker.store(true, Ordering::SeqCst);
        });
        supervisor.spawn(WorkerId::DecisionAudit, |_| async {});

        let result = supervise_pending(&mut supervisor).await;

        assert_eq!(
            result.unwrap_err(),
            WorkerFailure {
                worker: WorkerId::DecisionAudit,
                exit: WorkerTaskExit::Completed,
            }
        );
        assert!(sibling_stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn unexpected_worker_abort_fails_foreground_supervision() {
        let mut supervisor = WorkerSupervisor::new();
        supervisor.spawn(WorkerId::DecisionAudit, |_| async {
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        supervisor.abort_all();

        let result = supervise_pending(&mut supervisor).await;

        assert_eq!(
            result.unwrap_err(),
            WorkerFailure {
                worker: WorkerId::DecisionAudit,
                exit: WorkerTaskExit::Aborted,
            }
        );
    }

    #[tokio::test]
    async fn completed_foreground_drains_workers_without_fatal_error() {
        let mut supervisor = WorkerSupervisor::new();
        supervisor.spawn(WorkerId::ProfileRefresh, move |mut shutdown| async move {
            let _ = shutdown.changed().await;
        });

        assert!(supervisor.supervise_until(async {}).await.is_ok());
        assert!(supervisor.tasks.is_empty());
    }

    #[tokio::test]
    async fn drain_timeout_aborts_and_drops_stuck_workers() {
        struct ObserveDrop(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for ObserveDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut supervisor =
            WorkerSupervisor::with_drain_timeout(std::time::Duration::from_millis(10));
        let worker_dropped = Arc::clone(&dropped);
        supervisor.spawn(WorkerId::QrEvidence, move |_| async move {
            let _observe_drop = ObserveDrop(worker_dropped);
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;

        supervisor.shutdown_and_drain().await;

        assert!(dropped.load(Ordering::SeqCst));
        assert!(supervisor.tasks.is_empty());
    }
}
