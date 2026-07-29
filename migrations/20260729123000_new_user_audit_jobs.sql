create table if not exists new_user_audit_jobs (
    id bigserial primary key,
    chat_id bigint not null,
    telegram_user_id bigint not null,
    snapshot_hash text not null,
    prompt_version text not null,
    input_json jsonb not null,
    status text not null default 'pending'
        check (status in ('pending', 'processing', 'retry_wait', 'succeeded', 'failed')),
    attempts integer not null default 0 check (attempts >= 0),
    next_attempt_at timestamptz not null default now(),
    processing_started_at timestamptz,
    lease_expires_at timestamptz,
    error_kind text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (chat_id, telegram_user_id, snapshot_hash, prompt_version)
);

create index if not exists new_user_audit_jobs_ready_idx
    on new_user_audit_jobs (next_attempt_at, id)
    where status in ('pending', 'retry_wait');

create index if not exists new_user_audit_jobs_lease_idx
    on new_user_audit_jobs (lease_expires_at, id)
    where status = 'processing';
