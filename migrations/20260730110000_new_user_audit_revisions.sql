alter table telegram_new_user_profile_audits
    add column if not exists unified_audit_snapshot_hash text,
    add column if not exists unified_audit_generation bigint not null default 0;

alter table new_user_audit_jobs
    add column if not exists materialization_version text not null default 'unified-audit-materialization-v1',
    add column if not exists materialization_status text not null default 'pending'
        check (materialization_status in ('pending', 'succeeded', 'stale')),
    add column if not exists materialized_at timestamptz,
    add column if not exists materialization_error_kind text;

create index if not exists new_user_audit_jobs_materialization_idx
    on new_user_audit_jobs (materialization_status, materialization_version, id)
    where status = 'succeeded';
