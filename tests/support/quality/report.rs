use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidityClass {
    V1,
    V2,
    V3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseOrigin {
    PublicSyntheticCorpus,
    PropertyGenerated,
    MinimizedReplay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    TelegramBotApiV10_4,
    AuthorizedObservedFixture,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaseReport {
    pub case_id: &'static str,
    pub invariant_id: &'static str,
    pub validity_class: ValidityClass,
    pub origin: CaseOrigin,
    pub evidence_source: Option<EvidenceSource>,
    pub seed: Option<u64>,
    pub generated: u32,
    pub accepted: u32,
    pub rejected: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureInput<'a> {
    pub raw_payload: &'a str,
    pub report: CaseReport,
}

impl FailureInput<'_> {
    /// The serialized report intentionally contains metadata only. Payloads are
    /// accepted here only so callers can prove they are omitted from artifacts.
    pub fn safe_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.report).expect("case report serializes")
    }
}

impl CaseReport {
    pub fn is_well_formed(&self) -> bool {
        !self.case_id.is_empty()
            && !self.invariant_id.is_empty()
            && self.accepted.saturating_add(self.rejected) <= self.generated
            && (!matches!(
                self.origin,
                CaseOrigin::PropertyGenerated | CaseOrigin::MinimizedReplay
            ) || self.seed.is_some())
            && (self.validity_class != ValidityClass::V1 || self.evidence_source.is_some())
    }
}
