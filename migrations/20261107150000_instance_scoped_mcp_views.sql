-- Bind existing reviewed MCP projections to one explicitly provisioned
-- community scope per database. Historical scope migrations stay immutable.
create schema if not exists mcp_private;
revoke all on schema mcp_private from public;

create table if not exists mcp_private.instance_scope (
    scope_key boolean primary key default true check (scope_key),
    instance_id text not null check (length(btrim(instance_id)) > 0),
    discussion_chat_id bigint not null check (discussion_chat_id <> 0),
    source_channel_id bigint,
    provisioned_at timestamptz not null default now(),
    check (source_channel_id is null or (source_channel_id <> 0 and source_channel_id <> discussion_chat_id))
);

-- Upgrade compatibility: the previously reviewed production projection was
-- NedoNews. New community databases must be explicitly reprovisioned from the
-- selected community profile before an MCP server can start.
insert into mcp_private.instance_scope
    (scope_key, instance_id, discussion_chat_id, source_channel_id)
values
    (true, 'nedonews', -1001932061163, -1001575496091)
on conflict (scope_key) do nothing;

revoke all on mcp_private.instance_scope from public;

create or replace function mcp_private.matches_instance_scope(
    requested_instance_id text,
    requested_discussion_chat_id bigint,
    requested_source_channel_id bigint
) returns boolean
language sql
stable
security definer
set search_path = pg_catalog, mcp_private
as $$
    select (select count(*) from mcp_private.instance_scope where scope_key = true) = 1
       and exists (
           select 1
           from mcp_private.instance_scope
           where scope_key = true
             and instance_id = requested_instance_id
             and discussion_chat_id = requested_discussion_chat_id
             and source_channel_id is not distinct from requested_source_channel_id
       )
$$;
revoke all on function mcp_private.matches_instance_scope(text, bigint, bigint) from public;

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'nedobot_mcp_ro') then
        revoke all on mcp_private.instance_scope from nedobot_mcp_ro;
        execute 'grant execute on function mcp_private.matches_instance_scope(text, bigint, bigint) to nedobot_mcp_ro';
        execute 'grant usage on schema mcp_public to nedobot_mcp_ro';
        execute 'grant select on all tables in schema mcp_public to nedobot_mcp_ro';
        execute 'grant usage on schema mcp_private to nedobot_mcp_ro';
        execute 'grant select on mcp_private.telegram_media to nedobot_mcp_ro';
    end if;
end $$;

do $$
declare
    view_row record;
    scoped_definition text;
    has_scope_literal boolean;
    inherits_scoped_view boolean;
begin
    for view_row in
        select schemaname, viewname, definition
        from pg_views
        where schemaname in ('mcp_public', 'mcp_private')
        order by schemaname, viewname
    loop
        has_scope_literal := position('-1001932061163' in view_row.definition) > 0
            or position('-1001575496091' in view_row.definition) > 0;
        inherits_scoped_view := view_row.definition like '%mcp_public.telegram_chat_users%'
            or view_row.definition like '%mcp_public.telegram_user_profiles%';
        scoped_definition := replace(
            replace(
                view_row.definition,
                quote_literal('-1001932061163') || '::bigint',
                '(select discussion_chat_id from mcp_private.instance_scope where scope_key = true)'
            ),
            quote_literal('-1001575496091') || '::bigint',
            '(select source_channel_id from mcp_private.instance_scope where scope_key = true)'
        );

        -- A few retired profile-analysis views inherit scope from an already
        -- scoped user view. Everything else must carry an explicit scope.
        if not has_scope_literal
           and not (view_row.schemaname = 'mcp_public' and inherits_scoped_view) then
            raise exception 'MCP view %.% has no verifiable instance scope',
                view_row.schemaname, view_row.viewname;
        end if;

        if has_scope_literal then
            execute format(
                'create or replace view %I.%I as %s',
                view_row.schemaname,
                view_row.viewname,
                scoped_definition
            );
        end if;
    end loop;
end $$;

-- Fail the migration if any existing projection retained a fixed community ID.
do $$
declare
    unscoped_view text;
begin
    select format('%I.%I', schemaname, viewname)
    into unscoped_view
    from pg_views
    where schemaname in ('mcp_public', 'mcp_private')
      and (definition like '%-1001932061163%'
           or definition like '%-1001575496091%')
    limit 1;
    if unscoped_view is not null then
        raise exception 'MCP view % still contains a fixed community identifier', unscoped_view;
    end if;
end $$;
