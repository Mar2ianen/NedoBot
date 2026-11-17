use super::generators::{ActorRole, Capability, JobOperation, JobPhase};

/// Independent fixture model for the authorization contract used by tests.
/// It is deliberately a table, not a wrapper around a production permission API.
/// These generic capabilities are harness fixtures, not NedoBot policy; domain
/// tasks must derive their own expected matrix from the relevant product contract.
pub fn reference_allows(actor: ActorRole, capability: Capability) -> bool {
    matches!(
        (actor, capability),
        (ActorRole::Owner, _)
            | (ActorRole::Admin, Capability::Read | Capability::Moderate)
            | (ActorRole::Member, Capability::Read)
    )
}

/// Minimal reference state used by later tasks to compare PostgreSQL job
/// transitions. A claim advances generation; a completion or retry must match
/// the current claimed generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceJob {
    pub phase: JobPhase,
    pub attempts: u16,
}

impl ReferenceJob {
    pub fn pending() -> Self {
        Self {
            phase: JobPhase::Pending { generation: 0 },
            attempts: 0,
        }
    }

    pub fn apply(&mut self, operation: JobOperation) -> bool {
        match (self.phase, operation) {
            (JobPhase::Pending { generation }, JobOperation::Claim)
            | (JobPhase::Retryable { generation, .. }, JobOperation::Claim) => {
                let next = generation.saturating_add(1);
                self.phase = JobPhase::Claimed { generation: next };
                true
            }
            (
                JobPhase::Claimed {
                    generation: current,
                },
                JobOperation::Complete { generation },
            ) if current == generation => {
                self.phase = JobPhase::Completed { generation };
                true
            }
            (
                JobPhase::Claimed {
                    generation: current,
                },
                JobOperation::Retry { generation },
            ) if current == generation => {
                self.attempts = self.attempts.saturating_add(1);
                self.phase = JobPhase::Retryable {
                    generation,
                    attempts: self.attempts,
                };
                true
            }
            (JobPhase::Claimed { .. }, JobOperation::ExpireLease) => {
                let generation = match self.phase {
                    JobPhase::Claimed { generation } => generation,
                    _ => unreachable!("match arm requires claimed phase"),
                };
                self.phase = JobPhase::Retryable {
                    generation,
                    attempts: self.attempts,
                };
                true
            }
            _ => false,
        }
    }
}

/// An ambiguous acknowledgement must never be treated as a confirmed reject.
pub fn delivery_outcome_is_ambiguous(outcome: super::generators::HttpOutcome) -> bool {
    matches!(
        outcome,
        super::generators::HttpOutcome::Timeout
            | super::generators::HttpOutcome::DisconnectedAfterAcceptance
    )
}
