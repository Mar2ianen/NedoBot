use super::*;

pub(super) struct AuditPlan {
    pub(super) run_audit_job: bool,
    pub(super) same_fingerprint_in_flight: bool,
}

pub(super) struct AuditPlanner;

impl AuditPlanner {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn plan(
        features: &NewUserFeatures,
        risk: &RiskAnalysis,
        is_old_active_user: bool,
        avatar_changed: bool,
        fresh_content_changed: bool,
        link_evidence_changed: bool,
        needs_initial_audit: bool,
        trigger_fingerprint: Option<&str>,
        previous_trigger_fingerprint: Option<&(String, NewUserAuditStatus, bool)>,
    ) -> AuditPlan {
        let run_audit_job = should_enqueue_new_user_audit_job_with_link(
            features,
            risk,
            is_old_active_user,
            avatar_changed,
            fresh_content_changed,
            link_evidence_changed,
            needs_initial_audit,
        );
        let same_fingerprint_in_flight = match (trigger_fingerprint, previous_trigger_fingerprint) {
            (Some(current), Some((previous, status, _))) => {
                current == previous
                    && matches!(
                        *status,
                        NewUserAuditStatus::Pending
                            | NewUserAuditStatus::RetryWait
                            | NewUserAuditStatus::Processing
                    )
            }
            _ => false,
        };
        AuditPlan {
            run_audit_job,
            same_fingerprint_in_flight,
        }
    }
}

pub(super) fn analysis_config_from_runtime(config: &Config) -> NewUserAnalysisConfig {
    let Some(profile) = config.moderation_risk_profile() else {
        return NewUserAnalysisConfig::default();
    };
    let profile_name = config.community.moderation.risk_profile.trim();
    let id_model =
        profile_context::telegram_id_model_for_date(profile, chrono::Utc::now().date_naive());
    NewUserAnalysisConfig {
        old_user_message_threshold: profile.old_user_message_threshold,
        review_threshold: profile.review_threshold,
        risk_profile: profile_name.to_string(),
        risk_profile_version: if profile.version.trim().is_empty() {
            profile_name.to_string()
        } else {
            profile.version.clone()
        },
        telegram_id_model_version: id_model.as_ref().map(|model| model.version.clone()),
        telegram_id_model: id_model,
        ..NewUserAnalysisConfig::default()
    }
}

#[cfg(test)]
pub(super) fn should_enqueue_new_user_audit_job(
    features: &NewUserFeatures,
    risk: &RiskAnalysis,
    is_old_active_user: bool,
    avatar_changed: bool,
    fresh_content_changed: bool,
    needs_initial_audit: bool,
) -> bool {
    should_enqueue_new_user_audit_job_with_link(
        features,
        risk,
        is_old_active_user,
        avatar_changed,
        fresh_content_changed,
        false,
        needs_initial_audit,
    )
}

pub(super) fn record_campaign_snapshot(
    input: &mut Value,
    campaign: Option<&deterministic::CampaignMatch>,
) {
    input["work_contact_review"] = campaign
        .filter(|c| !c.can_short_circuit_audit())
        .map(deterministic::CampaignMatch::score_signal)
        .unwrap_or(Value::Null);
    let legacy = campaign.filter(|c| c.can_short_circuit_audit());
    input["deterministic_campaign_rule"] = legacy
        .map(|c| Value::String(c.rule.clone()))
        .unwrap_or(Value::Null);
    input["deterministic_campaign_evidence"] = legacy
        .map(deterministic::CampaignMatch::score_signal)
        .unwrap_or(Value::Null);
}

pub(super) fn deterministic_campaign_from_features(
    features: &NewUserFeatures,
) -> Option<deterministic::CampaignMatch> {
    deterministic::classify(
        features.first_message_text.as_deref(),
        features.message_count,
        features.chat_age_sec,
        features.personal_channel_has_adult_links,
    )
    .or_else(|| {
        features
            .recent_message_texts
            .iter()
            .take(5)
            .find_map(|text| {
                deterministic::classify(
                    Some(text),
                    features.message_count,
                    features.chat_age_sec,
                    false,
                )
                .filter(|campaign| !campaign.can_short_circuit_audit())
            })
    })
}

pub(super) fn should_enqueue_new_user_audit_job_with_link(
    features: &NewUserFeatures,
    risk: &RiskAnalysis,
    is_old_active_user: bool,
    avatar_changed: bool,
    fresh_content_changed: bool,
    link_evidence_changed: bool,
    needs_initial_audit: bool,
) -> bool {
    let has_deterministic_campaign = deterministic_campaign_from_features(features).is_some();
    has_deterministic_campaign
        || should_run_new_user_llm_audit(
            features,
            risk,
            is_old_active_user,
            avatar_changed,
            fresh_content_changed,
            link_evidence_changed,
            needs_initial_audit,
        )
}

fn should_run_new_user_llm_audit(
    features: &NewUserFeatures,
    risk: &RiskAnalysis,
    is_old_active_user: bool,
    avatar_changed: bool,
    fresh_content_changed: bool,
    link_evidence_changed: bool,
    needs_initial_audit: bool,
) -> bool {
    // Mature accounts do not need an LLM pass on every profile refresh. Keep
    // the pass when current activity carries evidence that can change the
    // decision: changed avatar/content evidence or a strong local score.
    // Profile metadata alone is useful only for genuinely new/low-activity accounts.
    let has_fresh_content_signal = avatar_changed || fresh_content_changed || link_evidence_changed;

    if is_old_active_user {
        return risk.score >= NEW_USER_LLM_AUDIT_MIN_BASELINE_SCORE || has_fresh_content_signal;
    }

    avatar_changed
        || needs_initial_audit
        || risk.score >= NEW_USER_LLM_AUDIT_MIN_BASELINE_SCORE
        || (fresh_content_changed
            && (has_long_first_comment(features)
                || features.personal_channel_has_adult_links
                || personal_channel_has_invite_link(features)
                || personal_channel_has_external_link(features)))
        || link_evidence_changed
}
