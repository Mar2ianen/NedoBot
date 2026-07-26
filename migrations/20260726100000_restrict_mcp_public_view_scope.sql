-- Keep all public MCP projections strictly within the discussion chat.
-- A matching source_channel_id alone can occur in a private or foreign chat.
-- PostgreSQL does not allow CREATE OR REPLACE VIEW to remove old view columns,
-- so recreate only these reviewed public projections explicitly.

drop view if exists mcp_public.search_runs;
drop view if exists mcp_public.llm_generations;
drop view if exists mcp_public.post_comment_jobs;
drop view if exists mcp_public.telegram_messages;

create view mcp_public.telegram_messages as
select id, chat_id, message_id, user_id, source_channel_id, source_message_id,
       is_automatic_forward, text, reply_to_message_id, reply_to_user_id,
       sender_chat_id, via_bot_id, has_photo, has_video, has_document, has_audio,
       has_voice, has_sticker, has_animation, has_links, created_at, updated_at,
       edited_at, edit_count, deleted_by_bot_at, deleted_by_bot_reason,
       deleted_by_bot_actor_id, spam_marked_at, spam_reason, spam_source,
       spam_marked_by_user_id, spam_type
from public.telegram_messages
where chat_id = -1001932061163;

create view mcp_public.post_comment_jobs as
select id, discussion_chat_id, discussion_message_id, source_channel_id,
       source_message_id, cleaned_post_text, status, error, bot_comment_message_id,
       created_at, updated_at
from public.post_comment_jobs
where discussion_chat_id = -1001932061163;

create view mcp_public.llm_generations as
select g.id, g.post_comment_job_id, g.provider, g.model, g.prompt, g.image_used,
       g.response, g.final_html, g.attempts, g.used_search_result_id, g.created_at
from public.llm_generations g
join public.post_comment_jobs j on j.id = g.post_comment_job_id
where j.discussion_chat_id = -1001932061163;

create view mcp_public.search_runs as
select r.id, r.post_comment_job_id, r.status, r.skipped_reason, r.latency_ms,
       r.queries, r.results, r.created_at
from public.search_runs r
join public.post_comment_jobs j on j.id = r.post_comment_job_id
where j.discussion_chat_id = -1001932061163;
