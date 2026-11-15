do $$
begin
    if exists (select 1 from pg_roles where rolname = 'nedobot_mcp_ro') then
        revoke all on table public.telegram_username_aliases from nedobot_mcp_ro;
    end if;
end
$$;
