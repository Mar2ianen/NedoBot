create table if not exists telegram_risk_captcha_challenges (
    id uuid primary key,
    chat_id bigint not null,
    telegram_user_id bigint not null,
    audit_job_id bigint not null,
    risk_score integer not null check (risk_score between 0 and 100),
    question text not null,
    options jsonb not null check (jsonb_typeof(options) = 'array'),
    correct_option smallint not null check (correct_option between 0 and 3),
    restore_permissions jsonb not null check (jsonb_typeof(restore_permissions) = 'object'),
    restriction_applied_at timestamptz,
    message_id integer,
    status text not null default 'preparing'
        check (status in ('preparing', 'setting_up', 'pending', 'solving', 'passed', 'failed', 'overridden', 'setup_failed')),
    attempts integer not null default 0 check (attempts >= 0),
    setup_attempts integer not null default 0 check (setup_attempts >= 0),
    setup_next_attempt_at timestamptz not null default now(),
    setup_lease_expires_at timestamptz,
    setup_error_kind text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    solved_at timestamptz,
    unique (chat_id, telegram_user_id)
);

create index if not exists telegram_risk_captcha_active_idx
    on telegram_risk_captcha_challenges (chat_id, status, created_at desc)
    where status in ('preparing', 'setting_up', 'pending', 'solving');

create index if not exists telegram_risk_captcha_setup_ready_idx
    on telegram_risk_captcha_challenges (setup_next_attempt_at, created_at)
    where status = 'preparing';

create index if not exists telegram_risk_captcha_setup_lease_idx
    on telegram_risk_captcha_challenges (setup_lease_expires_at, created_at)
    where status = 'setting_up';
