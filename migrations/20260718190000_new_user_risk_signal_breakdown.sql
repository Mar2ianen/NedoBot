alter table telegram_new_user_profile_audits
    add column if not exists risk_signal_breakdown jsonb not null default '[]'::jsonb;

create or replace view mcp_public.telegram_new_user_profile_audits as
select chat_id, telegram_user_id, analyzed_at, first_seen_at, last_seen_at,
       account_seen_age_sec, chat_age_sec, first_message_id, last_message_id,
       message_count, reply_count, link_count, media_count, voice_count,
       reply_to_channel_post_count, reply_to_bot_count, top_level_message_count,
       reply_to_comment_count, only_replies_or_comments, only_channel_post_comments,
       message_count_24h, link_count_24h, burst_messages_per_min,
       first_message_text, first_message_len, last_message_text, last_message_len,
       normalized_message_count, distinct_normalized_message_count,
       duplicate_normalized_message_count, max_normalized_message_reuse_count,
       max_pairwise_message_similarity, avg_message_len, repetitive_message_pattern,
       telegram_user_id_bucket, telegram_user_id_rank_ratio, telegram_user_id_is_recent,
       username, username_len, username_has_digits, username_digit_count,
       username_has_random_suffix, username_pattern, first_name, last_name,
       display_name, first_name_feminine_pattern, display_name_reuse_count,
       display_name_reuse_spammer_count, display_name_reused_by_spammers, is_bot,
       is_premium, language_code, bio, bio_len, has_profile_photo, profile_photo_count,
       profile_photo_reuse_count, profile_photo_file_unique_id, profile_photo_dc_id,
       profile_photo_dc_source, profile_photo_width, profile_photo_height,
       has_emoji_status, profile_accent_color_id, personal_channel_chat_id,
       personal_channel_title, personal_channel_username, personal_channel_message_count,
       personal_channel_last_message_id, personal_channel_last_message_at,
       personal_channel_last_text, personal_channel_has_adult_links,
       personal_channel_has_invite_link, personal_channel_has_external_link,
       personal_channel_title_len, personal_channel_last_text_len,
       personal_channel_refreshed_at, personal_channel_fetch_error, member_status,
       member_is_present, member_is_admin, join_event_seen,
       via_chat_folder_invite_link, risk_score, risk_level, primary_risk_class,
       risk_class_scores, risk_labels, risk_reasons, risk_signal_breakdown
from public.telegram_new_user_profile_audits where chat_id = -1001932061163;
