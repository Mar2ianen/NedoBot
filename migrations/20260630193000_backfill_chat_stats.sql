update telegram_messages
set reply_to_message_id = nullif(raw_json #>> '{reply_to_message,message_id}', '')::integer
where reply_to_message_id is null
  and raw_json #>> '{reply_to_message,message_id}' is not null;

update telegram_messages
set reply_to_user_id = nullif(raw_json #>> '{reply_to_message,from,id}', '')::bigint
where reply_to_user_id is null
  and raw_json #>> '{reply_to_message,from,id}' is not null;

update telegram_messages
set sender_chat_id = nullif(raw_json #>> '{sender_chat,id}', '')::bigint
where sender_chat_id is null
  and raw_json #>> '{sender_chat,id}' is not null;

update telegram_messages
set via_bot_id = nullif(raw_json #>> '{via_bot,id}', '')::bigint
where via_bot_id is null
  and raw_json #>> '{via_bot,id}' is not null;

update telegram_messages
set has_photo = raw_json ? 'photo',
    has_video = raw_json ? 'video',
    has_document = raw_json ? 'document',
    has_audio = raw_json ? 'audio',
    has_voice = raw_json ? 'voice',
    has_sticker = raw_json ? 'sticker',
    has_animation = raw_json ? 'animation',
    has_links = coalesce(text, '') like '%http://%'
        or coalesce(text, '') like '%https://%'
        or coalesce(text, '') like '%t.me/%'
        or raw_json @? '$.entities[*] ? (@.type == "url" || @.type == "text_link")'
        or raw_json @? '$.caption_entities[*] ? (@.type == "url" || @.type == "text_link")';

insert into telegram_user_profiles
    (telegram_user_id, username, first_name, last_name, is_bot, is_premium, language_code, last_seen_at, updated_at)
select distinct on ((raw_json #>> '{from,id}')::bigint)
    (raw_json #>> '{from,id}')::bigint,
    raw_json #>> '{from,username}',
    coalesce(raw_json #>> '{from,first_name}', ''),
    raw_json #>> '{from,last_name}',
    coalesce((raw_json #>> '{from,is_bot}')::boolean, false),
    nullif(raw_json #>> '{from,is_premium}', '')::boolean,
    raw_json #>> '{from,language_code}',
    created_at,
    now()
from telegram_messages
where raw_json #>> '{from,id}' is not null
order by (raw_json #>> '{from,id}')::bigint, created_at desc
on conflict (telegram_user_id) do update set
    username = excluded.username,
    first_name = excluded.first_name,
    last_name = excluded.last_name,
    is_bot = excluded.is_bot,
    is_premium = excluded.is_premium,
    language_code = excluded.language_code,
    last_seen_at = greatest(telegram_user_profiles.last_seen_at, excluded.last_seen_at),
    updated_at = now();
