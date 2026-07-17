-- Curated public projections for the unauthenticated NedoNews MCP endpoint.
-- New source columns/tables are deliberately absent until reviewed here.
create schema if not exists mcp_public;

create or replace view mcp_public.telegram_messages as
select id, chat_id, message_id, user_id, source_channel_id, source_message_id,
       is_automatic_forward, text, reply_to_message_id, reply_to_user_id,
       sender_chat_id, via_bot_id, has_photo, has_video, has_document, has_audio,
       has_voice, has_sticker, has_animation, has_links, created_at, updated_at,
       edited_at, edit_count, deleted_by_bot_at, deleted_by_bot_reason,
       deleted_by_bot_actor_id, spam_marked_at, spam_reason, spam_source,
       spam_marked_by_user_id, spam_type
from public.telegram_messages
where chat_id = -1001932061163
   or source_channel_id = -1001575496091;

create or replace view mcp_public.telegram_message_edits as
select id, chat_id, message_id, user_id, old_text, new_text, edited_at, observed_at
from public.telegram_message_edits where chat_id = -1001932061163;

create or replace view mcp_public.telegram_user_profiles as
select p.telegram_user_id, p.username, p.first_name, p.last_name, p.is_bot,
       p.is_premium, p.language_code, p.bio, p.profile_photo_file_unique_id,
       p.profile_photo_width, p.profile_photo_height, p.profile_photo_count,
       p.emoji_status_custom_emoji_id, p.profile_accent_color_id, p.last_seen_at,
       p.updated_at, p.profile_refreshed_at, p.profile_refresh_error,
       p.personal_channel_chat_id, p.personal_channel_title, p.personal_channel_username,
       p.personal_channel_message_count, p.personal_channel_last_message_id,
       p.personal_channel_last_message_at, p.personal_channel_last_text,
       p.personal_channel_has_adult_links, p.personal_channel_refreshed_at,
       p.personal_channel_fetch_error
from public.telegram_user_profiles p
where exists (select 1 from public.telegram_messages m
              where m.chat_id = -1001932061163 and m.user_id = p.telegram_user_id)
   or exists (select 1 from public.telegram_chat_users u
              where u.chat_id = -1001932061163 and u.telegram_user_id = p.telegram_user_id);

create or replace view mcp_public.telegram_chat_users as
select chat_id, telegram_user_id, first_seen_at, last_seen_at, first_message_id,
       last_message_id, message_count, reply_count, link_count, media_count,
       reply_to_channel_post_count, reply_to_bot_count, member_status, is_admin,
       is_present, member_observed_at, first_joined_at, last_joined_at, last_left_at,
       via_chat_folder_invite_link, is_spammer, spam_score, spam_message_count,
       spam_last_marked_at, spam_reason, spam_type, spam_types, spam_profile_labels,
       spam_profile_note, created_at, updated_at
from public.telegram_chat_users where chat_id = -1001932061163;

create or replace view mcp_public.telegram_chat_member_snapshots as
select chat_id, telegram_user_id, status, is_admin, is_present, observed_at
from public.telegram_chat_member_snapshots where chat_id = -1001932061163;

create or replace view mcp_public.telegram_chat_member_events as
select id, chat_id, telegram_user_id, actor_user_id, old_status, new_status,
       via_chat_folder_invite_link, event_at, created_at
from public.telegram_chat_member_events where chat_id = -1001932061163;

create or replace view mcp_public.telegram_message_reactions as
select id, chat_id, message_id, user_id, actor_chat_id, old_reactions, new_reactions,
       event_at, created_at
from public.telegram_message_reactions where chat_id = -1001932061163;

create or replace view mcp_public.telegram_message_reaction_counts as
select chat_id, message_id, reactions, total_count, event_at, updated_at
from public.telegram_message_reaction_counts where chat_id = -1001932061163;

create or replace view mcp_public.telegram_new_user_profile_audits as
select chat_id, telegram_user_id, analyzed_at, first_seen_at, last_seen_at,
       account_seen_age_sec, chat_age_sec, first_message_id, last_message_id,
       message_count, reply_count, link_count, media_count, voice_count,
       reply_to_channel_post_count, reply_to_bot_count, top_level_message_count,
       reply_to_comment_count, only_replies_or_comments, only_channel_post_comments,
       message_count_24h, link_count_24h, burst_messages_per_min, first_message_text,
       first_message_len, last_message_text, last_message_len, normalized_message_count,
       distinct_normalized_message_count, duplicate_normalized_message_count,
       max_normalized_message_reuse_count, max_pairwise_message_similarity,
       avg_message_len, repetitive_message_pattern, telegram_user_id_bucket,
       telegram_user_id_rank_ratio, telegram_user_id_is_recent, username, username_len,
       username_has_digits, username_digit_count, username_has_random_suffix,
       username_pattern, first_name, last_name, display_name, first_name_feminine_pattern,
       display_name_reuse_count, display_name_reuse_spammer_count,
       display_name_reused_by_spammers, is_bot, is_premium, language_code, bio, bio_len,
       has_profile_photo, profile_photo_count, profile_photo_reuse_count,
       profile_photo_file_unique_id, profile_photo_dc_id, profile_photo_dc_source,
       profile_photo_width, profile_photo_height, has_emoji_status,
       profile_accent_color_id, personal_channel_chat_id, personal_channel_title,
       personal_channel_username, personal_channel_message_count,
       personal_channel_last_message_id, personal_channel_last_message_at,
       personal_channel_last_text, personal_channel_has_adult_links,
       personal_channel_has_invite_link, personal_channel_has_external_link,
       personal_channel_title_len, personal_channel_last_text_len,
       personal_channel_refreshed_at, personal_channel_fetch_error, member_status,
       member_is_present, member_is_admin, join_event_seen,
       via_chat_folder_invite_link, risk_score, risk_level, primary_risk_class,
       risk_class_scores, risk_labels, risk_reasons
from public.telegram_new_user_profile_audits where chat_id = -1001932061163;

create or replace view mcp_public.post_comment_jobs as
select id, discussion_chat_id, discussion_message_id, source_channel_id,
       source_message_id, cleaned_post_text, status, error, bot_comment_message_id,
       created_at, updated_at
from public.post_comment_jobs
where discussion_chat_id = -1001932061163 or source_channel_id = -1001575496091;

create or replace view mcp_public.llm_generations as
select g.id, g.post_comment_job_id, g.provider, g.model, g.prompt, g.image_used,
       g.response, g.final_html, g.attempts, g.used_search_result_id, g.created_at
from public.llm_generations g
join public.post_comment_jobs j on j.id = g.post_comment_job_id
where j.discussion_chat_id = -1001932061163 or j.source_channel_id = -1001575496091;

create or replace view mcp_public.post_memory_notes as
select id, source_channel_id, source_message_id, title, summary, cautions, keywords,
       raw_note, merged_source_posts, last_source_channel_id, last_source_message_id,
       created_at, updated_at
from public.post_memory_notes where source_channel_id = -1001575496091;

create or replace view mcp_public.search_runs as
select r.id, r.post_comment_job_id, r.status, r.skipped_reason, r.latency_ms,
       r.queries, r.results, r.created_at
from public.search_runs r join public.post_comment_jobs j on j.id = r.post_comment_job_id
where j.discussion_chat_id = -1001932061163 or j.source_channel_id = -1001575496091;

create or replace view mcp_public.ask_runs as
select id, chat_id, command_message_id, requester_user_id, question,
       reply_to_message_id, provider, model, status, error_kind, step_count,
       tool_call_count, answer_markdown, created_at, completed_at
from public.ask_runs where chat_id = -1001932061163;

create or replace view mcp_public.ask_tool_calls as
select t.id, t.ask_run_id, t.step_number, t.tool_name, t.arguments, t.status,
       t.result_count, t.latency_ms, t.error_kind, t.created_at
from public.ask_tool_calls t join public.ask_runs r on r.id = t.ask_run_id
where r.chat_id = -1001932061163;

create or replace view mcp_public.telegram_user_notes as
select id, chat_id, telegram_user_id, note, created_by_user_id, source_ask_run_id,
       source_message_ids, status, created_at, retracted_at, retracted_by_user_id
from public.telegram_user_notes where chat_id = -1001932061163;

create or replace view mcp_public.telegram_chat_notes as
select id, chat_id, note, created_by_user_id, source_ask_run_id, status,
       created_at, retracted_at, retracted_by_user_id
from public.telegram_chat_notes where chat_id = -1001932061163;

create or replace view mcp_public.voice_transcription_jobs as
select id, chat_id, message_id, user_id, file_unique_id, media_kind, duration_sec,
       file_size, mime_type, status, error, asr_provider, asr_model, cleanup_provider,
       cleanup_model, raw_transcript, cleaned_text, render_mode, chapters_json,
       segments_json, final_html, created_at, updated_at
from public.voice_transcription_jobs where chat_id = -1001932061163;

create or replace view mcp_public.telegram_profile_identity_observations as
select o.telegram_user_id, o.snapshot_key, o.username_normalized,
       o.display_name_normalized, o.profile_photo_file_unique_id, o.first_seen_at,
       o.last_seen_at
from public.telegram_profile_identity_observations o
where exists (select 1 from public.telegram_chat_users u
              where u.chat_id = -1001932061163 and u.telegram_user_id = o.telegram_user_id);

create or replace view mcp_public.avatar_profile_assessments as
select a.telegram_user_id, a.profile_photo_file_unique_id, a.features_snapshot_hash,
       a.prompt_version, a.provider, a.model, a.input_hash, a.assessment_json, a.analyzed_at
from public.avatar_profile_assessments a
where exists (select 1 from public.telegram_chat_users u
              where u.chat_id = -1001932061163 and u.telegram_user_id = a.telegram_user_id);

create or replace view mcp_public.avatar_image_analyses as
select a.profile_photo_file_unique_id, a.prompt_version, a.provider, a.model,
       a.input_hash, a.observation_json, a.analyzed_at
from public.avatar_image_analyses a
where exists (select 1 from mcp_public.telegram_user_profiles p
              where p.profile_photo_file_unique_id = a.profile_photo_file_unique_id);

create or replace view mcp_public.avatar_analysis_jobs as
select id, telegram_user_id, profile_photo_file_unique_id, features_snapshot_hash,
       prompt_version, status, attempts, next_attempt_at, processing_started_at,
       lease_expires_at, provider, model, error_kind, features_json, created_at, updated_at
from public.avatar_analysis_jobs j
where exists (select 1 from public.telegram_chat_users u
              where u.chat_id = -1001932061163 and u.telegram_user_id = j.telegram_user_id);

create or replace view mcp_public.admin_events as
select id, telegram_user_id, telegram_chat_id, event_type, payload, created_at
from public.admin_events where telegram_chat_id = -1001932061163;

revoke all on all tables in schema mcp_public from public;
do $$
begin
    if exists (select 1 from pg_roles where rolname = 'nedobot_mcp_ro') then
        grant usage on schema mcp_public to nedobot_mcp_ro;
        grant select on all tables in schema mcp_public to nedobot_mcp_ro;
        revoke all on schema public from nedobot_mcp_ro;
    end if;
end $$;
