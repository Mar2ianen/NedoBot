use std::sync::atomic::{AtomicU64, Ordering};

/// Process-local counters for delivery decisions that need operator attention.
#[derive(Default)]
pub struct AskDeliveryMetrics {
    unknown_delivery_failures: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AskDeliveryMetricsSnapshot {
    pub unknown_delivery_failures: u64,
}

impl AskDeliveryMetrics {
    pub fn record_unknown_delivery_failure(&self) {
        self.unknown_delivery_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Returns a process-local snapshot for health/observability exporters.
    pub fn snapshot(&self) -> AskDeliveryMetricsSnapshot {
        AskDeliveryMetricsSnapshot {
            unknown_delivery_failures: self.unknown_delivery_failures.load(Ordering::Relaxed),
        }
    }
}
