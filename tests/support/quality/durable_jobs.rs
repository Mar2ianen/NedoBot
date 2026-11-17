use super::generators::DurableJobOperation;

const LEASE_SECONDS: i64 = 30;
const RETRY_SECONDS: i64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableJobStatus {
    Pending,
    Processing,
    RetryWait,
    Succeeded,
    Failed,
    DeliveryUnknown,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableJobModel {
    pub status: DurableJobStatus,
    pub fence: u64,
    pub attempts: u64,
    pub now: i64,
    pub next_attempt_at: i64,
    pub lease_expires_at: Option<i64>,
    pub owner: Option<u8>,
    pub external_effect_started: bool,
    pub confirmed_effects: u64,
}

impl Default for DurableJobModel {
    fn default() -> Self {
        Self {
            status: DurableJobStatus::Pending,
            fence: 0,
            attempts: 0,
            now: 0,
            next_attempt_at: 0,
            lease_expires_at: None,
            owner: None,
            external_effect_started: false,
            confirmed_effects: 0,
        }
    }
}

impl DurableJobModel {
    pub fn apply(&mut self, operation: DurableJobOperation) -> bool {
        match operation {
            DurableJobOperation::Claim { worker } => self.claim(worker),
            DurableJobOperation::Heartbeat { worker } if self.owns_live_lease(worker) => {
                self.lease_expires_at = Some(self.now + LEASE_SECONDS);
                true
            }
            DurableJobOperation::BeginExternalEffect { worker } if self.owns_live_lease(worker) => {
                self.external_effect_started = true;
                true
            }
            DurableJobOperation::FinalizeOk { worker } if self.owns_live_lease(worker) => {
                self.status = DurableJobStatus::Succeeded;
                self.confirmed_effects += 1;
                self.clear_claim();
                true
            }
            DurableJobOperation::FinalizeRetry { worker } if self.owns_live_lease(worker) => {
                self.status = DurableJobStatus::RetryWait;
                self.next_attempt_at = self.now + RETRY_SECONDS;
                self.external_effect_started = false;
                self.clear_claim();
                true
            }
            DurableJobOperation::FinalizeFailed { worker } if self.owns_live_lease(worker) => {
                self.status = DurableJobStatus::Failed;
                self.clear_claim();
                true
            }
            DurableJobOperation::MarkDeliveryUnknown { worker }
                if self.owns_live_lease(worker) && self.external_effect_started =>
            {
                self.status = DurableJobStatus::DeliveryUnknown;
                self.clear_claim();
                true
            }
            DurableJobOperation::AdvanceClock { seconds } => {
                self.now += i64::from(seconds);
                true
            }
            DurableJobOperation::Cancel
                if !matches!(
                    self.status,
                    DurableJobStatus::Succeeded
                        | DurableJobStatus::Failed
                        | DurableJobStatus::DeliveryUnknown
                        | DurableJobStatus::Cancelled
                ) =>
            {
                self.status = if self.external_effect_started {
                    DurableJobStatus::DeliveryUnknown
                } else {
                    DurableJobStatus::Cancelled
                };
                self.clear_claim();
                true
            }
            // Restart is a process event; persistent SQL state is unchanged.
            DurableJobOperation::Restart | DurableJobOperation::DuplicateIngress => false,
            _ => false,
        }
    }

    fn claim(&mut self, worker: u8) -> bool {
        let eligible = match self.status {
            DurableJobStatus::Pending => true,
            DurableJobStatus::RetryWait => self.now >= self.next_attempt_at,
            DurableJobStatus::Processing
                if self
                    .lease_expires_at
                    .is_some_and(|expires_at| self.now >= expires_at) =>
            {
                if self.external_effect_started {
                    self.status = DurableJobStatus::DeliveryUnknown;
                    self.clear_claim();
                    return false;
                }
                true
            }
            _ => false,
        };
        if !eligible {
            return false;
        }
        self.status = DurableJobStatus::Processing;
        self.owner = Some(worker);
        self.fence += 1;
        self.attempts += 1;
        self.lease_expires_at = Some(self.now + LEASE_SECONDS);
        self.external_effect_started = false;
        true
    }

    fn owns_live_lease(&self, worker: u8) -> bool {
        self.status == DurableJobStatus::Processing
            && self.owner == Some(worker)
            && self
                .lease_expires_at
                .is_some_and(|expires_at| self.now < expires_at)
    }

    fn clear_claim(&mut self) {
        self.owner = None;
        self.lease_expires_at = None;
    }

    pub fn invariant_holds(&self) -> bool {
        let claim_is_consistent = (self.status == DurableJobStatus::Processing)
            == (self.owner.is_some() && self.lease_expires_at.is_some());
        let attempts_cover_fences = self.attempts == self.fence;
        let unknown_has_no_confirmed_effect =
            self.status != DurableJobStatus::DeliveryUnknown || self.confirmed_effects == 0;
        claim_is_consistent && attempts_cover_fences && unknown_has_no_confirmed_effect
    }
}
