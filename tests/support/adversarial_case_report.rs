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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseReport {
    pub case_id: String,
    pub invariant_id: String,
    pub validity_class: ValidityClass,
    pub origin: CaseOrigin,
    pub seed: Option<u64>,
    pub generated: u32,
    pub accepted: u32,
    pub rejected: u32,
}

impl CaseReport {
    pub fn is_well_formed(&self) -> bool {
        !self.case_id.is_empty()
            && matches!(self.invariant_id.as_str(), "URL-01" | "URL-02" | "SYS-01")
            && self.accepted.saturating_add(self.rejected) <= self.generated
            && (self.origin != CaseOrigin::PropertyGenerated || self.seed.is_some())
    }
}

#[derive(Debug, Deserialize)]
pub struct SyntheticUrlCase {
    pub id: String,
    pub input: String,
    pub class: String,
    pub oracle_hint: String,
    pub validity: String,
}

#[derive(Debug, Deserialize)]
pub struct SyntheticUpdateCase {
    pub id: String,
    pub validity: String,
    pub note: String,
    pub update: serde_json::Value,
}
