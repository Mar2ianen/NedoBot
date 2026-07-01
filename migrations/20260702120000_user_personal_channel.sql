alter table telegram_user_profiles
    add column if not exists personal_channel_chat_id bigint,
    add column if not exists personal_channel_title text,
    add column if not exists personal_channel_username text,
    add column if not exists personal_channel_message_count integer,
    add column if not exists personal_channel_last_message_id integer,
    add column if not exists personal_channel_last_message_at timestamptz,
    add column if not exists personal_channel_last_text text,
    add column if not exists personal_channel_has_adult_links boolean not null default false,
    add column if not exists personal_channel_raw_json jsonb,
    add column if not exists personal_channel_refreshed_at timestamptz,
    add column if not exists personal_channel_fetch_error text;

create index if not exists telegram_user_profiles_personal_channel_idx
    on telegram_user_profiles (personal_channel_chat_id)
    where personal_channel_chat_id is not null;

create index if not exists telegram_user_profiles_personal_channel_adult_idx
    on telegram_user_profiles (personal_channel_has_adult_links, personal_channel_refreshed_at desc)
    where personal_channel_has_adult_links;
