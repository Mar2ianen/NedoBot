-- Keep recoverable materialization jobs runnable after the message-image
-- scorer version changes. Successfully materialized rows remain untouched.
update new_user_audit_jobs
set materialization_version = 'unified-audit-materialization-v4',
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
