alter table public.telegram_messages
    add column if not exists is_forwarded boolean not null default false;

update public.telegram_messages
set is_forwarded = (
    is_automatic_forward
    or raw_json -> 'forward_origin' is not null
    or nullif(raw_json ->> 'forwarded_from', '') is not null
    or nullif(raw_json ->> 'forwarded_from_id', '') is not null
)
where not is_forwarded;

create or replace view mcp_public.telegram_messages as
select id, chat_id, message_id, user_id, source_channel_id, source_message_id,
       is_automatic_forward, text, reply_to_message_id, reply_to_user_id,
       sender_chat_id, via_bot_id, has_photo, has_video, has_document, has_audio,
       has_voice, has_sticker, has_animation, has_links, created_at, updated_at,
       edited_at, edit_count, deleted_by_bot_at, deleted_by_bot_reason,
       deleted_by_bot_actor_id, spam_marked_at, spam_reason, spam_source,
       spam_marked_by_user_id, spam_type, is_forwarded,
       nullif(coalesce(
           nullif(raw_json #>> '{forward_origin,sender_user,username}', ''),
           nullif(concat_ws(' ',
               raw_json #>> '{forward_origin,sender_user,first_name}',
               raw_json #>> '{forward_origin,sender_user,last_name}'
           ), ''),
           nullif(raw_json #>> '{forward_origin,sender_chat,username}', ''),
           nullif(raw_json #>> '{forward_origin,sender_chat,title}', ''),
           nullif(raw_json #>> '{forward_origin,chat,username}', ''),
           nullif(raw_json #>> '{forward_origin,chat,title}', ''),
           nullif(raw_json ->> 'forwarded_from', ''),
           nullif(raw_json ->> 'forwarded_from_id', '')
       ), '') as forwarded_from
from public.telegram_messages
where chat_id = -1001932061163;
