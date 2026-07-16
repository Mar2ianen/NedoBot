create table if not exists telegram_chat_notes (
    id bigserial primary key,
    chat_id bigint not null,
    note text not null,
    created_by_user_id bigint not null,
    source_ask_run_id bigint references ask_runs(id) on delete set null,
    status text not null default 'active',
    created_at timestamptz not null default now(),
    retracted_at timestamptz,
    retracted_by_user_id bigint,
    check (status in ('active', 'retracted'))
);

create index if not exists telegram_chat_notes_active_idx
    on telegram_chat_notes (chat_id, created_at desc)
    where status = 'active';
