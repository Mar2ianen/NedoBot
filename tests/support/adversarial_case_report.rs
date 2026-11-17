pub use crate::quality::report::{CaseOrigin, CaseReport, ValidityClass};
use serde::Deserialize;

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
