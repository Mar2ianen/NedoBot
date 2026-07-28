/// Result of a finalization guarded by the claim attempt.
///
/// A worker may finish after its lease was reclaimed. Domain repositories must
/// treat that result as stale and avoid applying follow-up side effects.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CasResult {
    Applied,
    LeaseLost,
}

impl CasResult {
    pub fn from_rows_affected(rows_affected: u64) -> anyhow::Result<Self> {
        match rows_affected {
            0 => Ok(Self::LeaseLost),
            1 => Ok(Self::Applied),
            _ => anyhow::bail!("job finalization unexpectedly affected multiple rows"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CasResult;

    #[test]
    fn maps_expected_cas_update_counts() {
        assert_eq!(
            CasResult::from_rows_affected(0).unwrap(),
            CasResult::LeaseLost
        );
        assert_eq!(
            CasResult::from_rows_affected(1).unwrap(),
            CasResult::Applied
        );
        assert!(CasResult::from_rows_affected(2).is_err());
    }
}
