#[derive(Debug, Clone, Copy)]
pub struct LeasePolicy {
    seconds: i64,
}

impl LeasePolicy {
    pub const fn new(seconds: i64) -> Self {
        Self { seconds }
    }

    pub const fn seconds(self) -> i64 {
        self.seconds
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WorkerPollPolicy {
    idle_seconds: u64,
    error_seconds: u64,
}

impl WorkerPollPolicy {
    pub const fn new(idle_seconds: u64, error_seconds: u64) -> Self {
        Self {
            idle_seconds,
            error_seconds,
        }
    }

    pub const fn idle_seconds(self) -> u64 {
        self.idle_seconds
    }

    pub const fn error_seconds(self) -> u64 {
        self.error_seconds
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    delays_seconds: &'static [i64],
}

impl RetryPolicy {
    pub const fn new(delays_seconds: &'static [i64]) -> Self {
        Self { delays_seconds }
    }

    /// `retry_ordinal` starts at one for the first consecutive retryable failure.
    /// It is independent from a job's monotonic claim sequence used for CAS.
    pub fn delay_seconds(
        self,
        retry_ordinal: i32,
        retry_after_seconds: Option<i64>,
    ) -> Option<i64> {
        let attempt_index = usize::try_from(retry_ordinal.checked_sub(1)?).ok()?;
        let scheduled_delay = *self.delays_seconds.get(attempt_index)?;
        Some(scheduled_delay.max(retry_after_seconds.unwrap_or_default().max(0)))
    }
}

pub const EXTERNAL_REQUEST_LEASE: LeasePolicy = LeasePolicy::new(10 * 60);
pub const CHAT_EMBEDDING_LEASE: LeasePolicy = LeasePolicy::new(10 * 60);
pub const CHAT_EMBEDDING_RETRY: RetryPolicy = RetryPolicy::new(&[15, 30, 60, 120]);
pub const ANALYSIS_RETRY: RetryPolicy = RetryPolicy::new(&[15, 30, 60, 5 * 60, 24 * 60 * 60]);
pub const EXTERNAL_ANALYSIS_POLL: WorkerPollPolicy = WorkerPollPolicy::new(5, 5);

#[cfg(test)]
mod tests {
    use super::{
        ANALYSIS_RETRY, CHAT_EMBEDDING_LEASE, CHAT_EMBEDDING_RETRY, EXTERNAL_ANALYSIS_POLL,
        EXTERNAL_REQUEST_LEASE,
    };

    #[test]
    fn analysis_retry_has_bounded_schedule() {
        assert_eq!(ANALYSIS_RETRY.delay_seconds(1, None), Some(15));
        assert_eq!(ANALYSIS_RETRY.delay_seconds(5, None), Some(86_400));
        assert_eq!(ANALYSIS_RETRY.delay_seconds(6, None), None);
        assert_eq!(ANALYSIS_RETRY.delay_seconds(0, None), None);
    }

    #[test]
    fn retry_after_never_shortens_schedule() {
        assert_eq!(ANALYSIS_RETRY.delay_seconds(1, Some(5)), Some(15));
        assert_eq!(ANALYSIS_RETRY.delay_seconds(1, Some(75)), Some(75));
    }

    #[test]
    fn named_leases_and_polls_preserve_existing_timing() {
        assert_eq!(EXTERNAL_REQUEST_LEASE.seconds(), 600);
        assert_eq!(CHAT_EMBEDDING_LEASE.seconds(), 600);
        assert_eq!(CHAT_EMBEDDING_RETRY.delay_seconds(1, None), Some(15));
        assert_eq!(CHAT_EMBEDDING_RETRY.delay_seconds(4, None), Some(120));
        assert_eq!(CHAT_EMBEDDING_RETRY.delay_seconds(5, None), None);
        assert_eq!(EXTERNAL_ANALYSIS_POLL.idle_seconds(), 5);
        assert_eq!(EXTERNAL_ANALYSIS_POLL.error_seconds(), 5);
    }
}
