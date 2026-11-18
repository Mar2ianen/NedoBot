//! Converts URL relation observations into the existing advisory shadow signal.
//! This projection never authorizes moderation or production scoring.

use serde_json::Value;
use teloxide_antispam::url_shadow::{UrlShadowAnalysis, UrlShadowEvidence, analyze_url_shadow};

pub(crate) struct UrlShadowProjection;

impl UrlShadowProjection {
    pub(crate) fn project(relationships: &Value, promotional_content: bool) -> UrlShadowAnalysis {
        analyze_url_shadow(UrlShadowEvidence {
            unique_external_url_count: relationships["unique_external_url_count"]
                .as_u64()
                .unwrap_or_default()
                .min(u16::MAX as u64) as u16,
            promotional_content,
            verified_threat_feed_hit: relationships["verified_threat_feed_hit"]
                .as_bool()
                .unwrap_or(false),
            ..UrlShadowEvidence::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn projection_keeps_shadow_advisory_inputs_typed_and_source_neutral() {
        let analysis = UrlShadowProjection::project(
            &json!({
                "unique_external_url_count": 3,
                "verified_threat_feed_hit": true,
                "source": "qr"
            }),
            true,
        );

        assert_eq!(analysis.suggested_delta, 32);
        assert_eq!(
            analysis
                .signals
                .iter()
                .map(|signal| signal.label)
                .collect::<Vec<_>>(),
            [
                "verified_url_threat_feed_hit",
                "promotion_with_external_url"
            ]
        );
    }

    #[test]
    fn malformed_observation_fails_closed_to_no_url_signal() {
        let analysis =
            UrlShadowProjection::project(&json!({"unique_external_url_count":"1"}), false);
        assert_eq!(analysis.suggested_delta, 0);
        assert!(analysis.signals.is_empty());
    }
}
