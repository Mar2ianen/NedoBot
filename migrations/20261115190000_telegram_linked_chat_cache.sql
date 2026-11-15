-- Only verified Bot API relationships for channels already configured by the
-- operator are stored here. No user-controlled username is materialized.
create table public.telegram_linked_chat_cache (
    source_channel_id bigint primary key,
    linked_chat_id bigint,
    observed_at timestamptz not null,
    checked_at timestamptz not null default now()
);

create index telegram_linked_chat_cache_fresh_idx
    on public.telegram_linked_chat_cache (observed_at desc)
    where linked_chat_id is not null;

-- Separate, aggregate-only shadow evidence. It is deliberately absent from
-- the LLM audit input, baseline risk columns, and public MCP projections.
create table public.telegram_url_shadow_observations (
    chat_id bigint not null,
    telegram_user_id bigint not null,
    source_fingerprint char(64) not null,
    observed_at timestamptz not null default now(),
    shadow_score smallint not null,
    evidence jsonb not null check (jsonb_typeof(evidence) = 'object'),
    primary key (chat_id, telegram_user_id)
);

create index telegram_url_shadow_observations_recent_idx
    on public.telegram_url_shadow_observations (chat_id, observed_at desc);

do $$ begin
    if exists (select 1 from pg_roles where rolname = 'nedobot_mcp_ro') then
        revoke all on table public.telegram_linked_chat_cache from nedobot_mcp_ro;
        revoke all on table public.telegram_url_shadow_observations from nedobot_mcp_ro;
    end if;
end $$;
