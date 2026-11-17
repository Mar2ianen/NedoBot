use super::generators::{ActorRole, Capability, JobOperation, JobPhase};
use super::generators::{EvidenceRef, EvidenceSource, IngressMessageKey, IngressOperation};
use std::collections::{BTreeMap, BTreeSet};

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

/// Independent expected projection for one synthetic message key. This model
/// describes desired state transitions and does not call the SQL service.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReferenceIngress {
    pub revision: Option<u16>,
    pub media_generation: Option<u8>,
    pub message_evidence: BTreeSet<EvidenceRef>,
    pub qr_targets: BTreeSet<u8>,
    pub shadow_revision: Option<u16>,
    pub shadow_evidence: BTreeSet<EvidenceRef>,
}

impl ReferenceIngress {
    pub fn apply(&mut self, operation: &IngressOperation) {
        match operation {
            IngressOperation::Receive {
                revision,
                media_generation,
                evidence,
            } => {
                if self.revision.is_none_or(|current| *revision > current) {
                    self.revision = Some(*revision);
                    self.media_generation = Some(*media_generation);
                    self.message_evidence = evidence.iter().cloned().collect();
                    self.qr_targets.clear();
                }
            }
            IngressOperation::Duplicate { .. } => {}
            IngressOperation::Edit {
                revision,
                media_generation,
                evidence,
            } if self.revision.is_none_or(|current| *revision > current) => {
                let media_changed = self.media_generation != Some(*media_generation);
                self.revision = Some(*revision);
                self.media_generation = Some(*media_generation);
                self.message_evidence = evidence.iter().cloned().collect();
                if media_changed {
                    self.qr_targets.clear();
                }
            }
            IngressOperation::Edit { .. } => {}
            IngressOperation::RemoveLink { revision }
                if self.revision.is_none_or(|current| *revision > current) =>
            {
                self.revision = Some(*revision);
                self.message_evidence.clear();
            }
            IngressOperation::RemoveLink { .. } => {}
            IngressOperation::AttachQr {
                media_generation,
                targets,
            }
            | IngressOperation::RetryOldJob {
                media_generation,
                targets,
            } if self.media_generation == Some(*media_generation) => {
                self.qr_targets = targets.iter().copied().collect();
            }
            IngressOperation::AttachQr { .. } | IngressOperation::RetryOldJob { .. } => {}
            IngressOperation::ReplaceMedia {
                revision,
                media_generation,
            } if self.revision.is_none_or(|current| *revision > current) => {
                self.revision = Some(*revision);
                self.media_generation = Some(*media_generation);
                self.qr_targets.clear();
            }
            IngressOperation::ReplaceMedia { .. } => {}
            IngressOperation::DeleteStoredMessage => {
                *self = Self::default();
            }
            IngressOperation::RefreshShadow { revision } if self.revision == Some(*revision) => {
                self.shadow_revision = Some(*revision);
                self.shadow_evidence = self.current_evidence();
            }
            IngressOperation::RefreshShadow { .. } => {}
        }
    }

    pub fn current_evidence(&self) -> BTreeSet<EvidenceRef> {
        let mut evidence = self.message_evidence.clone();
        evidence.extend(self.qr_targets.iter().copied().map(|target| EvidenceRef {
            target,
            source: EvidenceSource::Qr,
        }));
        evidence
    }

    pub fn unique_target_count(&self) -> usize {
        self.current_evidence()
            .into_iter()
            .map(|evidence| evidence.target)
            .collect::<BTreeSet<_>>()
            .len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReferenceIngressStore {
    messages: BTreeMap<IngressMessageKey, ReferenceIngress>,
}

impl ReferenceIngressStore {
    pub fn apply(&mut self, key: IngressMessageKey, operation: &IngressOperation) {
        if matches!(operation, IngressOperation::DeleteStoredMessage) {
            self.messages.remove(&key);
            return;
        }
        self.messages.entry(key).or_default().apply(operation);
    }

    pub fn message(&self, key: &IngressMessageKey) -> Option<&ReferenceIngress> {
        self.messages.get(key)
    }
}
