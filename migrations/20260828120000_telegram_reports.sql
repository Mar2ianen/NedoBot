create table if not exists telegram_reports (
    id bigserial primary key,
    chat_id bigint not null,
    message_id integer not null,
    reporter_user_id bigint not null,
    reported_user_id bigint not null,
    reason text not null default '',
    target_text text,
    target_media text not null default 'text',
    target_reply_to_message_id integer,
    target_created_at timestamptz not null,
    reporter_snapshot jsonb not null default '{}'::jsonb,
    target_snapshot jsonb not null default '{}'::jsonb,
    status text not null default 'pending'
        check (status in ('pending', 'sent', 'partial', 'failed')),
    resolution text not null default 'pending'
        check (resolution in ('pending', 'accepted', 'rejected')),
    resolved_by_user_id bigint,
    resolved_at timestamptz,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (chat_id, message_id)
);

create index if not exists telegram_reports_reporter_created_idx
    on telegram_reports (reporter_user_id, created_at desc);

create index if not exists telegram_reports_status_idx
    on telegram_reports (status, created_at desc);

create table if not exists telegram_report_deliveries (
    report_id bigint not null references telegram_reports(id) on delete cascade,
    admin_user_id bigint not null,
    status text not null default 'pending'
        check (status in ('pending', 'processing', 'sent', 'unreachable', 'failed')),
    attempt_count integer not null default 0,
    next_attempt_at timestamptz not null default now(),
    lease_expires_at timestamptz,
    telegram_message_id integer,
    error_kind text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (report_id, admin_user_id)
);

create index if not exists telegram_report_deliveries_claim_idx
    on telegram_report_deliveries (status, next_attempt_at, report_id);
