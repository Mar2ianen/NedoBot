alter table telegram_user_profiles
    add column if not exists bio text,
    add column if not exists profile_photo_small_file_id text,
    add column if not exists profile_photo_small_file_unique_id text,
    add column if not exists profile_photo_big_file_id text,
    add column if not exists profile_photo_big_file_unique_id text,
    add column if not exists profile_photo_file_id text,
    add column if not exists profile_photo_file_unique_id text,
    add column if not exists profile_photo_width integer,
    add column if not exists profile_photo_height integer,
    add column if not exists profile_photo_count integer,
    add column if not exists emoji_status_custom_emoji_id text,
    add column if not exists profile_accent_color_id smallint,
    add column if not exists profile_raw_json jsonb,
    add column if not exists profile_refreshed_at timestamptz,
    add column if not exists profile_refresh_error text;

create index if not exists telegram_user_profiles_profile_refreshed_idx
    on telegram_user_profiles (profile_refreshed_at nulls first);

create index if not exists telegram_user_profiles_photo_unique_idx
    on telegram_user_profiles (profile_photo_file_unique_id)
    where profile_photo_file_unique_id is not null;
