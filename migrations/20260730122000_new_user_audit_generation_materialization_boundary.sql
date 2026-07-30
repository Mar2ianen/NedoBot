-- P2: an assessment_json is authoritative once generation succeeded. Legacy rows
-- that retained one while a generation retry was scheduled must be replayed only
-- by the separate materialization lifecycle, never sent to the LLM again.
update new_user_audit_jobs
set status = 'succeeded',
    completed_at = coalesce(completed_at, now()),
    error_kind = null,
    processing_started_at = null,
    lease_expires_at = null,
    materialization_status = case
        when materialization_status = 'processing'
             and materialization_lease_expires_at > now() then 'processing'
        when status = 'retry_wait' then 'retry_wait'
        else 'pending'
    end,
    materialization_next_attempt_at = case
        when materialization_status = 'processing'
             and materialization_lease_expires_at > now()
            then materialization_next_attempt_at
        when status = 'retry_wait' then next_attempt_at
        else now()
    end,
    materialization_processing_started_at = case
        when materialization_status = 'processing'
             and materialization_lease_expires_at > now()
            then materialization_processing_started_at
        else null
    end,
    materialization_lease_expires_at = case
        when materialization_status = 'processing'
             and materialization_lease_expires_at > now()
            then materialization_lease_expires_at
        else null
    end,
    materialization_error_kind = case
        when materialization_status = 'processing'
             and materialization_lease_expires_at > now()
            then materialization_error_kind
        when status = 'retry_wait' then coalesce(error_kind, 'legacy_generation_retry')
        else null
    end,
    updated_at = now()
where assessment_json is not null
  and status in ('processing', 'retry_wait');
