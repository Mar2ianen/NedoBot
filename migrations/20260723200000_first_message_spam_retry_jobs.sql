create table if not exists first_message_spam_analysis_jobs (
    id bigserial primary key,
    chat_id bigint not null,
    telegram_user_id bigint not null,
    status text not null default 'pending'
        check (status in ('pending', 'processing', 'retry_wait', 'succeeded', 'failed')),
    attempts integer not null default 0,
    next_attempt_at timestamptz not null default now(),
    processing_started_at timestamptz,
    lease_expires_at timestamptz,
    error_kind text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (chat_id, telegram_user_id)
);

create index if not exists first_message_spam_analysis_jobs_ready_idx
    on first_message_spam_analysis_jobs (next_attempt_at, id)
    where status in ('pending', 'retry_wait');
