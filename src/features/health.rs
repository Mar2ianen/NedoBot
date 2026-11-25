//! Bounded process-local health observations for /ping.
//! Observations are never credentials, tool payloads or raw provider errors.
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use sqlx::PgPool;
use tokio::{sync::watch, task::JoinSet};

use crate::{config::Config, features::jobs::observability::load_job_lifecycle_report};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Checking,
    Healthy,
    Degraded,
    Unavailable,
    TimedOut,
    Unknown,
    Disabled,
}

impl HealthStatus {
    pub const fn text(self) -> &'static str {
        match self {
            Self::Checking => "CHECKING",
            Self::Healthy => "OK",
            Self::Degraded => "DEGRADED",
            Self::Unavailable => "UNAVAILABLE",
            Self::TimedOut => "TIMEOUT",
            Self::Unknown => "UNKNOWN",
            Self::Disabled => "DISABLED",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckSample {
    pub status: HealthStatus,
    pub detail: String,
    pub elapsed_ms: Option<u64>,
    pub observed_at: Instant,
}

#[derive(Debug, Clone)]
pub struct WorkerSample {
    pub started_at: Instant,
    pub last_poll_at: Option<Instant>,
    pub last_poll_ms: Option<u64>,
    pub processed: u64,
    pub idle: u64,
    pub failures: u64,
    pub consecutive_failures: u64,
    pub stopped: bool,
}

impl WorkerSample {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            last_poll_at: None,
            last_poll_ms: None,
            processed: 0,
            idle: 0,
            failures: 0,
            consecutive_failures: 0,
            stopped: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HealthSnapshot {
    pub uptime: Duration,
    pub checks: BTreeMap<&'static str, CheckSample>,
    pub workers: BTreeMap<&'static str, WorkerSample>,
    pub busy: bool,
    pub revision: u64,
}

pub struct HealthRegistry {
    started: Instant,
    checks: Mutex<BTreeMap<&'static str, CheckSample>>,
    workers: Mutex<BTreeMap<&'static str, WorkerSample>>,
    collecting: AtomicBool,
    mcp_collecting: AtomicBool,
    revision: AtomicU64,
    changes: watch::Sender<u64>,
}

static LIVE: OnceLock<Arc<HealthRegistry>> = OnceLock::new();

impl Default for HealthRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthRegistry {
    pub fn new() -> Self {
        let (changes, _) = watch::channel(0);
        Self {
            started: Instant::now(),
            checks: Mutex::new(BTreeMap::new()),
            workers: Mutex::new(BTreeMap::new()),
            collecting: AtomicBool::new(false),
            mcp_collecting: AtomicBool::new(false),
            revision: AtomicU64::new(0),
            changes,
        }
    }

    pub fn install_global(registry: Arc<Self>) {
        // A production bot runs exactly one community instance per process.
        let _ = LIVE.set(registry);
    }

    pub fn global() -> Option<&'static Arc<Self>> {
        LIVE.get()
    }

    fn notify(&self) {
        let revision = self.revision.fetch_add(1, Ordering::Relaxed) + 1;
        self.changes.send_replace(revision);
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub fn snapshot(&self) -> HealthSnapshot {
        HealthSnapshot {
            uptime: self.started.elapsed(),
            checks: self.checks.lock().unwrap_or_else(|err| err.into_inner()).clone(),
            workers: self.workers.lock().unwrap_or_else(|err| err.into_inner()).clone(),
            busy: self.is_busy(),
            revision: self.revision.load(Ordering::Relaxed),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.collecting.load(Ordering::Acquire) || self.mcp_collecting.load(Ordering::Acquire)
    }

    fn set_check(
        &self,
        name: &'static str,
        status: HealthStatus,
        detail: impl Into<String>,
        elapsed_ms: Option<u64>,
    ) {
        self.checks.lock().unwrap_or_else(|err| err.into_inner()).insert(
            name,
            CheckSample {
                status,
                detail: detail.into(),
                elapsed_ms,
                observed_at: Instant::now(),
            },
        );
        self.notify();
    }

    pub fn observed(&self, name: &'static str, ok: bool, elapsed: Duration) {
        self.set_check(
            name,
            if ok { HealthStatus::Healthy } else { HealthStatus::Degraded },
            if ok { "last operation succeeded" } else { "last operation failed" },
            Some(milliseconds(elapsed)),
        );
    }

    pub fn register_worker(&self, name: &'static str) {
        self.workers.lock().unwrap_or_else(|err| err.into_inner())
            .insert(name, WorkerSample::new());
        self.notify();
    }

    pub fn exit_worker(&self, name: &'static str) {
        if let Some(sample) = self.workers.lock().unwrap_or_else(|err| err.into_inner()).get_mut(name) {
            sample.stopped = true;
        }
        self.notify();
    }

    pub fn worker_polled(&self, name: &'static str, outcome: WorkerOutcome, elapsed: Duration) {
        let mut workers = self.workers.lock().unwrap_or_else(|err| err.into_inner());
        let sample = workers.entry(name).or_insert_with(WorkerSample::new);
        sample.last_poll_at = Some(Instant::now());
        sample.last_poll_ms = Some(milliseconds(elapsed));
        match outcome {
            WorkerOutcome::Processed => {
                sample.processed = sample.processed.saturating_add(1);
                sample.consecutive_failures = 0;
            }
            WorkerOutcome::Idle => {
                sample.idle = sample.idle.saturating_add(1);
                sample.consecutive_failures = 0;
            }
            WorkerOutcome::Failed => {
                sample.failures = sample.failures.saturating_add(1);
                sample.consecutive_failures = sample.consecutive_failures.saturating_add(1);
            }
        }
        drop(workers);
        self.notify();
    }

    pub fn refresh(self: &Arc<Self>, pool: PgPool, config: Arc<Config>) {
        if self.collecting.swap(true, Ordering::AcqRel) {
            return;
        }
        for name in ["postgres", "queues", "ask", "voice", "moderation", "qr"] {
            self.set_check(name, HealthStatus::Checking, "refreshing", None);
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            this.refresh_inner(pool, config).await;
            this.collecting.store(false, Ordering::Release);
            this.notify();
        });
    }

    async fn refresh_inner(self: &Arc<Self>, pool: PgPool, config: Arc<Config>) {
        let mut jobs = JoinSet::<(&'static str, HealthStatus, String, u64)>::new();
        let p = pool.clone();
        jobs.spawn(async move {
            let started = Instant::now();
            let result = tokio::time::timeout(
                Duration::from_secs(1),
                sqlx::query_scalar::<_, i64>("select 1::bigint").fetch_one(&p),
            ).await;
            let (status, detail) = match result {
                Ok(Ok(1)) => (HealthStatus::Healthy,
                    format!("connections={} idle={}", p.size(), p.num_idle())),
                Ok(Ok(_)) | Ok(Err(_)) => (HealthStatus::Unavailable, "database query failed".to_owned()),
                Err(_) => (HealthStatus::TimedOut, "database deadline exceeded".to_owned()),
            };
            ("postgres", status, detail, milliseconds(started.elapsed()))
        });

        let p = pool.clone();
        jobs.spawn(async move {
            let started = Instant::now();
            let result = tokio::time::timeout(
                Duration::from_secs(3), load_job_lifecycle_report(&p)
            ).await;
            let (status, detail) = match result {
                Ok(Ok(queues)) => {
                    let mut all = Vec::with_capacity(queues.len());
                    let mut degraded = false;
                    for q in queues {
                        let queued: i64 = q.statuses.iter()
                            .filter(|row| matches!(row.status.as_str(), "pending" | "retry" | "retry_wait"))
                            .map(|row| row.jobs).sum();
                        let due_age = q.oldest_ready_age_seconds.unwrap_or(0.0).max(0.0);
                        if q.expired_lease_count > 0 || due_age > 600.0 { degraded = true; }
                        all.push(format!("{}={} queued oldest_due={}s expired={}",
                            q.queue, queued, due_age as u64, q.expired_lease_count));
                    }
                    (if degraded { HealthStatus::Degraded } else { HealthStatus::Healthy }, all.join("; "))
                }
                Ok(Err(_)) => (HealthStatus::Unavailable, "queue metrics unavailable".to_owned()),
                Err(_) => (HealthStatus::TimedOut, "queue metrics deadline exceeded".to_owned()),
            };
            ("queues", status, detail, milliseconds(started.elapsed()))
        });

        let p = pool.clone();
        let chat_id = config.discussion_chat_id;
        jobs.spawn(async move {
            let started = Instant::now();
            let result = tokio::time::timeout(Duration::from_secs(2),
                sqlx::query_as::<_, (i64, i64, i64)>(
                    "select count(*) filter (where status='completed')::bigint, \
                     count(*) filter (where status='failed')::bigint, \
                     count(*) filter (where status='running')::bigint \
                     from ask_runs where chat_id=$1 and created_at>now()-interval '1 hour'"
                ).bind(chat_id).fetch_one(&p)).await;
            let (status, detail) = match result {
                Ok(Ok((done, failed, running))) => (
                    if failed > 0 { HealthStatus::Degraded } else { HealthStatus::Healthy },
                    format!("1h completed={done} failed={failed} recorded_running={running}")
                ),
                Ok(Err(_)) => (HealthStatus::Unavailable, "ask metrics query failed".to_owned()),
                Err(_) => (HealthStatus::TimedOut, "ask metrics deadline exceeded".to_owned()),
            };
            ("ask", status, detail, milliseconds(started.elapsed()))
        });

        let p = pool.clone();
        jobs.spawn(async move {
            let started = Instant::now();
            let result = tokio::time::timeout(Duration::from_secs(2),
                sqlx::query_as::<_, (i64, i64, i64)>(
                    "select count(*) filter (where status in ('pending','retry_wait') and next_attempt_at<=now())::bigint, \
                     count(*) filter (where status='failed')::bigint, \
                     count(*) filter (where status='delivery_unknown')::bigint from voice_transcription_jobs"
                ).fetch_one(&p)).await;
            let (status, detail) = match result {
                Ok(Ok((ready, failed, unknown))) => (
                    if ready > 20 || unknown > 0 { HealthStatus::Degraded } else { HealthStatus::Healthy },
                    format!("ready={ready} failed_total={failed} delivery_unknown={unknown}")
                ),
                Ok(Err(_)) => (HealthStatus::Unavailable, "voice queue metrics failed".to_owned()),
                Err(_) => (HealthStatus::TimedOut, "voice queue deadline exceeded".to_owned()),
            };
            ("voice", status, detail, milliseconds(started.elapsed()))
        });

        let p = pool.clone();
        let instance = config.community.instance.id.clone();
        let chat_id = config.discussion_chat_id;
        jobs.spawn(async move {
            let started = Instant::now();
            let result = tokio::time::timeout(Duration::from_secs(2),
                sqlx::query_as::<_, (i64, i64)>(
                    "select \
                     (select count(*) from moderation_decision_jobs where instance_id=$1 and status in ('pending','retry_wait') and next_attempt_at<=now())::bigint, \
                     (select count(*) from new_user_audit_jobs where chat_id=$2 and status in ('pending','retry_wait') and next_attempt_at<=now())::bigint"
                ).bind(&instance).bind(chat_id).fetch_one(&p)).await;
            let (status, detail) = match result {
                Ok(Ok((decision, new_users))) => (
                    if decision > 20 || new_users > 20 { HealthStatus::Degraded } else { HealthStatus::Healthy },
                    format!("decision_ready={decision} new_user_ready={new_users}")
                ),
                Ok(Err(_)) => (HealthStatus::Unavailable, "moderation queue metrics failed".to_owned()),
                Err(_) => (HealthStatus::TimedOut, "moderation queue deadline exceeded".to_owned()),
            };
            ("moderation", status, detail, milliseconds(started.elapsed()))
        });

        let p = pool;
        let instance = config.community.instance.id.clone();
        let chat_id = config.discussion_chat_id;
        jobs.spawn(async move {
            let started = Instant::now();
            let result = tokio::time::timeout(Duration::from_secs(2),
                sqlx::query_as::<_, (i64, i64)>(
                    "select \
                    (select count(*) from telegram_qr_scan_jobs where chat_id=$1 and status='pending' and retry_at<=now())::bigint, \
                    (select count(*) from telegram_url_shadow_refresh_jobs where instance_id=$2 and status='pending' and retry_at<=now())::bigint"
                ).bind(chat_id).bind(&instance).fetch_one(&p)).await;
            let (status, detail) = match result {
                Ok(Ok((qr, shadow))) => (
                    if qr > 50 || shadow > 50 { HealthStatus::Degraded } else { HealthStatus::Healthy },
                    format!("qr_ready={qr} url_shadow_ready={shadow}")
                ),
                Ok(Err(_)) => (HealthStatus::Unavailable, "QR queue metrics failed".to_owned()),
                Err(_) => (HealthStatus::TimedOut, "QR queue deadline exceeded".to_owned()),
            };
            ("qr", status, detail, milliseconds(started.elapsed()))
        });

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !jobs.is_empty() {
            tokio::select! {
                joined = jobs.join_next() => {
                    if let Some(Ok((id, status, detail, elapsed))) = joined {
                        self.set_check(id, status, detail, Some(elapsed));
                    }
                }
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        jobs.abort_all();
        for id in ["postgres", "queues", "ask", "voice", "moderation", "qr"] {
            let timed_out = self.checks.lock().unwrap_or_else(|err| err.into_inner())
                .get(id).is_some_and(|c| c.status == HealthStatus::Checking);
            if timed_out {
                self.set_check(id, HealthStatus::TimedOut, "global refresh deadline exceeded", None);
            }
        }
    }

    #[cfg(feature = "ask")]
    pub fn probe_agent_mcp(self: &Arc<Self>, config: Arc<Config>) {
        if self.mcp_collecting.swap(true, Ordering::AcqRel) {
            return;
        }
        self.set_check("agent_mcp", HealthStatus::Checking, "RMCP initialize + tools/list", None);
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let started = Instant::now();
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                crate::features::ask::mcp_client::McpClient::start(&config),
            ).await;
            let (status, detail) = match result {
                Ok(Ok(client)) => {
                    let count = client.tool_names().count();
                    drop(client); // child transport uses kill_on_drop
                    (HealthStatus::Healthy, format!("initialize/tools/list OK; tools={count}"))
                }
                Ok(Err(_)) => (HealthStatus::Unavailable, "RMCP bootstrap failed".to_owned()),
                Err(_) => (HealthStatus::TimedOut, "RMCP bootstrap deadline exceeded".to_owned()),
            };
            this.set_check("agent_mcp", status, detail, Some(milliseconds(started.elapsed())));
            this.mcp_collecting.store(false, Ordering::Release);
            this.notify();
        });
    }
}

#[derive(Clone, Copy)]
pub enum WorkerOutcome {
    Processed,
    Idle,
    Failed,
}

pub fn worker_registered(name: &'static str) {
    if let Some(registry) = HealthRegistry::global() {
        registry.register_worker(name);
    }
}

pub fn worker_exited(name: &'static str) {
    if let Some(registry) = HealthRegistry::global() {
        registry.exit_worker(name);
    }
}

pub fn worker_polled(name: &'static str, outcome: WorkerOutcome, elapsed: Duration) {
    if let Some(registry) = HealthRegistry::global() {
        registry.worker_polled(name, outcome, elapsed);
    }
}

pub fn observe(name: &'static str, ok: bool, elapsed: Duration) {
    if let Some(registry) = HealthRegistry::global() {
        registry.observed(name, ok, elapsed);
    }
}

pub fn milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_distinct_and_do_not_invent_success() {
        assert_ne!(HealthStatus::TimedOut, HealthStatus::Healthy);
        assert_ne!(HealthStatus::Unknown, HealthStatus::Disabled);
    }

    #[test]
    fn worker_failures_reset_only_after_a_successful_poll() {
        let health = HealthRegistry::new();
        health.register_worker("synthetic");
        health.worker_polled("synthetic", WorkerOutcome::Failed, Duration::from_millis(9));
        health.worker_polled("synthetic", WorkerOutcome::Failed, Duration::from_millis(19));
        assert_eq!(health.snapshot().workers["synthetic"].consecutive_failures, 2);
        health.worker_polled("synthetic", WorkerOutcome::Idle, Duration::from_millis(1));
        assert_eq!(health.snapshot().workers["synthetic"].consecutive_failures, 0);
    }

    #[test]
    fn observation_records_milliseconds_and_age() {
        let health = HealthRegistry::new();
        health.observed("fake", true, Duration::from_micros(2750));
        let sample = &health.snapshot().checks["fake"];
        assert_eq!(sample.elapsed_ms, Some(2));
        assert_eq!(sample.status, HealthStatus::Healthy);
    }
}
