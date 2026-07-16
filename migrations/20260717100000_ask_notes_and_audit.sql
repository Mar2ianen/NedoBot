create table if not exists ask_runs (
    id bigserial primary key,
    chat_id bigint not null,
    command_message_id integer not null,
    requester_user_id bigint not null,
    question text not null,
    reply_to_message_id integer,
    provider text,
    model text,
    status text not null,
    error_kind text,
    step_count integer not null default 0,
    tool_call_count integer not null default 0,
    answer_markdown text,
    created_at timestamptz not null default now(),
    completed_at timestamptz
);

create index if not exists ask_runs_chat_created_idx on ask_runs (chat_id, created_at desc);

create table if not exists ask_tool_calls (
    id bigserial primary key,
    ask_run_id bigint not null references ask_runs(id) on delete cascade,
    step_number integer not null,
    tool_name text not null,
    arguments jsonb not null default '{}'::jsonb,
    status text not null,
    result_count integer,
    latency_ms bigint,
    error_kind text,
    created_at timestamptz not null default now()
);

create index if not exists ask_tool_calls_run_idx on ask_tool_calls (ask_run_id, step_number);

create table if not exists telegram_user_notes (
    id bigserial primary key,
    chat_id bigint not null,
    telegram_user_id bigint not null references telegram_user_profiles(telegram_user_id) on delete cascade,
    note text not null,
    created_by_user_id bigint not null,
    source_ask_run_id bigint references ask_runs(id) on delete set null,
    status text not null default 'active',
    created_at timestamptz not null default now(),
    retracted_at timestamptz,
    retracted_by_user_id bigint,
    check (status in ('active', 'retracted'))
);

create index if not exists telegram_user_notes_active_idx
    on telegram_user_notes (chat_id, telegram_user_id, created_at desc)
    where status = 'active';
