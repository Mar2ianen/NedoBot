-- Older deployments applied the original view before used_chat_message_ids was
-- added to its migration source. CREATE OR REPLACE VIEW can only append a new
-- column, so preserve the deployed order and add the reviewed provenance field
-- only where it is missing.
do $$
begin
    if not exists (
        select 1
        from information_schema.columns
        where table_schema = 'mcp_public'
          and table_name = 'llm_generations'
          and column_name = 'used_chat_message_ids'
    ) then
        execute $view$
            create or replace view mcp_public.llm_generations as
            select g.id, g.post_comment_job_id, g.provider, g.model, g.prompt,
                   g.image_used, g.response, g.final_html, g.attempts,
                   g.used_search_result_id, g.created_at, g.used_chat_message_ids
            from public.llm_generations g
            join public.post_comment_jobs j on j.id = g.post_comment_job_id
            where j.discussion_chat_id = -1001932061163
        $view$;
    end if;

    if exists (select 1 from pg_roles where rolname = 'nedobot_mcp_ro') then
        grant usage on schema mcp_public to nedobot_mcp_ro;
        grant select on all tables in schema mcp_public to nedobot_mcp_ro;
    end if;
end $$;
