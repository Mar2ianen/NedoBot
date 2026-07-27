-- Recreating reviewed public views drops their existing ACL entries.
-- Restore the least-privilege MCP role after scope migrations without creating it
-- on installations where the public MCP endpoint is intentionally disabled.
do $$
begin
    if exists (select 1 from pg_roles where rolname = 'nedobot_mcp_ro') then
        grant usage on schema mcp_public to nedobot_mcp_ro;
        grant select on all tables in schema mcp_public to nedobot_mcp_ro;
        revoke all on schema public from nedobot_mcp_ro;
    end if;
end $$;
