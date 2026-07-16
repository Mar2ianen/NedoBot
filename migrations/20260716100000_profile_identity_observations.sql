create table if not exists telegram_profile_identity_observations (
    telegram_user_id bigint not null references telegram_user_profiles (telegram_user_id) on delete cascade,
    snapshot_key text not null,
    username_normalized text,
    display_name_normalized text,
    profile_photo_file_unique_id text,
    first_seen_at timestamptz not null default now(),
    last_seen_at timestamptz not null default now(),
    primary key (telegram_user_id, snapshot_key)
);

create index if not exists telegram_profile_identity_observations_avatar_idx
    on telegram_profile_identity_observations (profile_photo_file_unique_id)
    where profile_photo_file_unique_id is not null;

create index if not exists telegram_profile_identity_observations_username_idx
    on telegram_profile_identity_observations (username_normalized)
    where username_normalized is not null;

create index if not exists telegram_profile_identity_observations_display_name_idx
    on telegram_profile_identity_observations (display_name_normalized)
    where display_name_normalized is not null;
