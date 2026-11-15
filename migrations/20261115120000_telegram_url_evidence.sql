create table public.telegram_message_url_evidence (
    chat_id bigint not null,
    message_id integer not null,
    parser_version smallint not null,
    item_index smallint not null,
    kind text not null check (kind in ('url', 'mention', 'user_id', 'hashtag', 'cashtag', 'bot_command')),
    source text not null check (source in ('visible_text', 'telegram_entity', 'rich_message', 'qr')),
    target_sha256 char(64) not null,
    host text,
    display_hint text not null,
    privacy_sensitive boolean not null,
    created_at timestamptz not null default now(),
    primary key (chat_id, message_id, parser_version, source, item_index),
    foreign key (chat_id, message_id)
        references public.telegram_messages (chat_id, message_id)
        on delete cascade
);

create table public.telegram_message_url_evidence_runs (
    chat_id bigint not null,
    message_id integer not null,
    parser_version smallint not null,
    source_fingerprint char(64) not null,
    target_count smallint not null check (target_count between 0 and 128),
    processed_at timestamptz not null default now(),
    primary key (chat_id, message_id, parser_version),
    foreign key (chat_id, message_id)
        references public.telegram_messages (chat_id, message_id)
        on delete cascade
);

create index telegram_message_url_evidence_target_sha256_idx
    on public.telegram_message_url_evidence (target_sha256);
create index telegram_message_url_evidence_chat_created_idx
    on public.telegram_message_url_evidence (chat_id, created_at desc);
create index telegram_message_url_evidence_runs_pending_idx
    on public.telegram_message_url_evidence_runs (chat_id, message_id);

create table public.telegram_qr_scan_jobs (
    id bigserial primary key,
    chat_id bigint not null,
    message_id integer not null,
    file_id text not null,
    file_unique_id text not null,
    status text not null default 'pending'
        check (status in ('pending', 'leased', 'succeeded', 'failed')),
    attempts integer not null default 0 check (attempts >= 0),
    lease_generation integer not null default 0 check (lease_generation >= 0),
    lease_expires_at timestamptz,
    retry_at timestamptz not null default now(),
    reference_count smallint not null default 0 check (reference_count between 0 and 64),
    error_kind text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (chat_id, message_id, file_unique_id),
    foreign key (chat_id, message_id)
        references public.telegram_messages (chat_id, message_id)
        on delete cascade
);

create index telegram_qr_scan_jobs_ready_idx
    on public.telegram_qr_scan_jobs (retry_at, id)
    where status in ('pending', 'leased');

do $$ begin
    if exists (select 1 from pg_roles where rolname = 'nedobot_mcp_ro') then
        revoke all on table public.telegram_qr_scan_jobs from nedobot_mcp_ro;
        revoke all on table public.telegram_message_url_evidence from nedobot_mcp_ro;
        revoke all on table public.telegram_message_url_evidence_runs from nedobot_mcp_ro;
    end if;
end $$;
