-- Preserve the reviewed semantic profile field without exposing member raw_json.
-- The projection is additive: CREATE OR REPLACE VIEW may append columns safely.
create or replace view mcp_public.telegram_chat_member_snapshots as
select chat_id,
       telegram_user_id,
       status,
       is_admin,
       is_present,
       observed_at,
       nullif(raw_json ->> 'custom_title', '') as admin_title
from public.telegram_chat_member_snapshots
where chat_id = -1001932061163;

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'nedobot_mcp_ro') then
        grant usage on schema mcp_public to nedobot_mcp_ro;
        grant select on mcp_public.telegram_chat_member_snapshots to nedobot_mcp_ro;
    end if;
end $$;
