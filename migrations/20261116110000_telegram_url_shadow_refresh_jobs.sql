create table public.telegram_url_shadow_refresh_jobs (
    id bigserial primary key,
    instance_id text not null check (length(instance_id) between 1 and 96),
    chat_id bigint not null,
    telegram_user_id bigint not null,
    evidence_revision char(64) not null,
    status text not null default 'pending'
        check (status in ('pending', 'leased', 'completed', 'failed')),
    attempts integer not null default 0 check (attempts >= 0),
    lease_generation integer not null default 0 check (lease_generation >= 0),
    lease_expires_at timestamptz,
    retry_at timestamptz not null default now(),
    error_kind text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (instance_id, chat_id, telegram_user_id, evidence_revision)
);

create index telegram_url_shadow_refresh_jobs_ready_idx
    on public.telegram_url_shadow_refresh_jobs (instance_id, retry_at, id)
    where status in ('pending', 'leased');

do $$ begin
    if exists (select 1 from pg_roles where rolname = 'nedobot_mcp_ro') then
        revoke all on table public.telegram_url_shadow_refresh_jobs from nedobot_mcp_ro;
    end if;
end $$;
