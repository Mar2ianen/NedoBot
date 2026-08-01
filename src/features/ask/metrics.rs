use std::sync::atomic::{AtomicU64, Ordering};

/// Process-local counters for delivery decisions that need operator attention.
#[derive(Default)]
pub struct AskDeliveryMetrics {
    unknown_delivery_failures: AtomicU64,
}

impl AskDeliveryMetrics {
    pub fn record_unknown_delivery_failure(&self) {
        self.unknown_delivery_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub fn unknown_delivery_failures(&self) -> u64 {
        self.unknown_delivery_failures.load(Ordering::Relaxed)
    }
}
