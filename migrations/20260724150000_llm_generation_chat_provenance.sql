alter table llm_generations
    add column if not exists used_chat_message_ids integer[] not null default '{}';

create or replace view mcp_public.llm_generations as
select g.id, g.post_comment_job_id, g.provider, g.model, g.prompt, g.image_used,
       g.response, g.final_html, g.attempts, g.used_search_result_id,
       g.used_chat_message_ids, g.created_at
from public.llm_generations g
join public.post_comment_jobs j on j.id = g.post_comment_job_id
where j.discussion_chat_id = -1001932061163 or j.source_channel_id = -1001575496091;
