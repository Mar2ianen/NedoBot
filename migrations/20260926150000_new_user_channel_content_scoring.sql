alter table telegram_new_user_profile_audits
    add column if not exists risk_personal_channel_score integer not null default 0
        check (risk_personal_channel_score >= 0),
    add column if not exists risk_personal_channel_signals jsonb not null default '[]'::jsonb;

comment on column telegram_new_user_profile_audits.risk_personal_channel_score is
    'Evidence-backed risk contribution from the latest available personal-channel content; channel attachment alone contributes zero.';

-- Requeue durable assessments that were waiting, retrying, or terminally stale
-- under the prior materializer. This revives reviews lost to score constraints
-- without regenerating their LLM assessments.
update new_user_audit_jobs
set materialization_version = 'unified-audit-materialization-v3',
    materialization_status = case
        when materialization_status in ('processing', 'stale') then 'retry_wait'
        else materialization_status
    end,
    materialization_attempts = 0,
    materialization_next_attempt_at = now(),
    materialization_processing_started_at = null,
    materialization_lease_expires_at = null,
    materialization_error_kind = null,
    materialized_at = null,
    updated_at = now()
where status = 'succeeded'
  and assessment_json is not null
  and materialization_status in ('pending', 'retry_wait', 'processing', 'stale');
