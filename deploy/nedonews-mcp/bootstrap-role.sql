-- Run as a PostgreSQL administrator, never commit the password.
-- Example: psql -v mcp_password='generated-secret' -f bootstrap-role.sql tg_ai_bot
\if :{?mcp_password}
\else
\quit
\endif

do $$
begin
    create role nedobot_mcp_ro login nosuperuser nocreatedb nocreaterole noinherit noreplication;
exception when duplicate_object then null;
end $$;

alter role nedobot_mcp_ro password :'mcp_password';
alter role nedobot_mcp_ro set default_transaction_read_only = on;
alter role nedobot_mcp_ro set statement_timeout = '5s';
alter role nedobot_mcp_ro set lock_timeout = '1s';
alter role nedobot_mcp_ro set idle_in_transaction_session_timeout = '5s';
grant connect on database tg_ai_bot to nedobot_mcp_ro;
revoke all on schema public from nedobot_mcp_ro;
grant usage on schema mcp_public to nedobot_mcp_ro;
grant select on all tables in schema mcp_public to nedobot_mcp_ro;
