drop view if exists mcp_public.post_memory_notes;
drop table if exists public.post_memory_notes;

create extension if not exists vector;

create table public.post_history_entries (
    id bigserial primary key,
    post_comment_job_id bigint not null unique
        references public.post_comment_jobs(id) on delete cascade,
    source_channel_id bigint not null,
    source_message_id integer not null,
    post_text text not null,
    bot_comment text not null,
    used_search_result jsonb,
    summary text,
    entities text[] not null default '{}',
    used_angle text,
    external_fact text,
    external_source_url text,
    skip_reason text,
    status text not null default 'pending'
        check (status in ('pending', 'processing', 'ready', 'ignored', 'retry')),
    attempts integer not null default 0,
    next_attempt_at timestamptz not null default now(),
    processing_started_at timestamptz,
    provider text,
    model text,
    embedding vector(312),
    embedding_model text,
    error_kind text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (source_channel_id, source_message_id),
    check ((status = 'ready') = (summary is not null and embedding is not null)),
    check (status <> 'ignored' or summary is null)
);

create index post_history_entries_claim_idx
    on public.post_history_entries (next_attempt_at, created_at)
    where status in ('pending', 'retry');

create index post_history_entries_ready_idx
    on public.post_history_entries (created_at desc)
    where status = 'ready';

create or replace view mcp_public.post_history_entries as
select id,
       post_comment_job_id,
       source_channel_id,
       source_message_id,
       summary,
       entities,
       used_angle,
       external_fact,
       external_source_url,
       skip_reason,
       status,
       provider,
       model,
       embedding_model,
       created_at,
       updated_at
from public.post_history_entries
where source_channel_id = -1001575496091;

revoke all on mcp_public.post_history_entries from public;
do $$
begin
    if exists (select 1 from pg_roles where rolname = 'nedobot_mcp_ro') then
        grant usage on schema mcp_public to nedobot_mcp_ro;
        grant select on mcp_public.post_history_entries to nedobot_mcp_ro;
    end if;
end $$;
