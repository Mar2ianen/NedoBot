//! Приватная диагностика persisted Decision, отдельно от старого profile audit.
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStage {
    NotEnqueued,
    Pending,
    Failed,
    Stale,
    Abstain,
    NoSignal,
    BelowThreshold,
    ActionPending,
    Policy,
    Delivery,
    Issued,
    Unknown,
}

/// Выбирается последняя сохранённая job по ID, а не по порядку JSON aggregation.
/// Отсутствие job не доказывает историческую причину отсутствия enqueue.
pub fn latest_job(decisions: &Value) -> Option<&Value> {
    decisions
        .as_array()?
        .iter()
        .filter(|job| job["id"].as_i64().is_some())
        .max_by_key(|job| job["id"].as_i64())
}

pub fn stage(job: Option<&Value>) -> DecisionStage {
    let Some(job) = job else {
        return DecisionStage::NotEnqueued;
    };
    match job["status"].as_str() {
        Some("pending" | "processing" | "retry_wait") => DecisionStage::Pending,
        Some("failed") => DecisionStage::Failed,
        Some("stale") => DecisionStage::Stale,
        Some("succeeded") => match job["shadow_verdict"].as_str() {
            Some("abstain") => DecisionStage::Abstain,
            Some("no_signal") => DecisionStage::NoSignal,
            Some("review_candidate") => action_stage(job),
            _ => DecisionStage::Unknown,
        },
        _ => DecisionStage::Unknown,
    }
}

fn action_stage(job: &Value) -> DecisionStage {
    match job["action_status"].as_str() {
        Some("pending" | "processing" | "retry_wait") => DecisionStage::ActionPending,
        Some("failed") => DecisionStage::Delivery,
        Some("not_requested") => DecisionStage::Policy,
        Some("done" | "skipped") => match job["action_error_kind"].as_str() {
            Some("issued") => DecisionStage::Issued,
            Some("below_threshold_or_abstain") => DecisionStage::BelowThreshold,
            Some("source_changed" | "source_missing" | "stale_metrics") => DecisionStage::Stale,
            Some(
                "captcha_dry_run" | "message_too_old" | "obsolete_policy" | "ineligible"
                | "existing_challenge",
            ) => DecisionStage::Policy,
            _ => DecisionStage::Unknown,
        },
        _ => DecisionStage::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn persisted_decision_separates_model_score_and_action_outcomes() {
        for (status, verdict, action, reason, expected) in [
            ("pending", "", "not_requested", "", DecisionStage::Pending),
            (
                "retry_wait",
                "",
                "not_requested",
                "budget",
                DecisionStage::Pending,
            ),
            (
                "failed",
                "",
                "not_requested",
                "http_400",
                DecisionStage::Failed,
            ),
            (
                "stale",
                "",
                "not_requested",
                "expired_or_changed",
                DecisionStage::Stale,
            ),
            (
                "succeeded",
                "no_signal",
                "done",
                "below_threshold_or_abstain",
                DecisionStage::NoSignal,
            ),
            (
                "succeeded",
                "abstain",
                "done",
                "below_threshold_or_abstain",
                DecisionStage::Abstain,
            ),
            (
                "succeeded",
                "review_candidate",
                "done",
                "below_threshold_or_abstain",
                DecisionStage::BelowThreshold,
            ),
            (
                "succeeded",
                "review_candidate",
                "pending",
                "",
                DecisionStage::ActionPending,
            ),
            (
                "succeeded",
                "review_candidate",
                "not_requested",
                "",
                DecisionStage::Policy,
            ),
            (
                "succeeded",
                "review_candidate",
                "done",
                "captcha_dry_run",
                DecisionStage::Policy,
            ),
            (
                "succeeded",
                "review_candidate",
                "skipped",
                "message_too_old",
                DecisionStage::Policy,
            ),
            (
                "succeeded",
                "review_candidate",
                "failed",
                "retry_exhausted",
                DecisionStage::Delivery,
            ),
            (
                "succeeded",
                "review_candidate",
                "done",
                "issued",
                DecisionStage::Issued,
            ),
            ("new_status", "", "", "", DecisionStage::Unknown),
            (
                "succeeded",
                "review_candidate",
                "done",
                "new_reason",
                DecisionStage::Unknown,
            ),
        ] {
            let job = json!({"status":status,"shadow_verdict":verdict,"action_status":action,"action_error_kind":reason});
            assert_eq!(stage(Some(&job)), expected, "{job}");
        }
        assert_eq!(stage(None), DecisionStage::NotEnqueued);
    }

    #[test]
    fn latest_job_is_independent_of_aggregation_order_and_current_account_age() {
        let rows = json!([
            {"id":12,"status":"succeeded","shadow_verdict":"abstain"},
            {"id":10,"status":"failed"}
        ]);
        let mut reversed = rows.as_array().unwrap().clone();
        reversed.reverse();
        for decisions in [rows, json!(reversed)] {
            let job = latest_job(&decisions).unwrap();
            assert_eq!(job["id"], 12);
            assert_eq!(stage(Some(job)), DecisionStage::Abstain);
        }
        assert!(latest_job(&Value::Null).is_none());
        assert!(latest_job(&json!([])).is_none());
    }
}
