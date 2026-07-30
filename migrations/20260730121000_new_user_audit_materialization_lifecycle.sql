alter table new_user_audit_jobs
    add column if not exists materialization_attempts integer not null default 0
        check (materialization_attempts >= 0),
    add column if not exists materialization_next_attempt_at timestamptz not null default now(),
    add column if not exists materialization_processing_started_at timestamptz,
    add column if not exists materialization_lease_expires_at timestamptz;

alter table new_user_audit_jobs
    drop constraint if exists new_user_audit_jobs_materialization_status_check;

alter table new_user_audit_jobs
    add constraint new_user_audit_jobs_materialization_status_check
    check (materialization_status in ('pending', 'processing', 'retry_wait', 'succeeded', 'stale'));

create index if not exists new_user_audit_jobs_materialization_ready_idx
    on new_user_audit_jobs (materialization_next_attempt_at, id)
    where status = 'succeeded'
      and materialization_status in ('pending', 'retry_wait');

create index if not exists new_user_audit_jobs_materialization_lease_idx
    on new_user_audit_jobs (materialization_lease_expires_at, id)
    where status = 'succeeded'
      and materialization_status = 'processing';
