create table if not exists telegram_new_user_profile_audits (
    chat_id bigint not null,
    telegram_user_id bigint not null,
    analyzed_at timestamptz not null default now(),

    first_seen_at timestamptz,
    last_seen_at timestamptz,
    account_seen_age_sec bigint,
    chat_age_sec bigint,
    first_message_id integer,
    last_message_id integer,
    message_count bigint not null default 0,
    reply_count bigint not null default 0,
    link_count bigint not null default 0,
    media_count bigint not null default 0,
    voice_count bigint not null default 0,
    reply_to_channel_post_count bigint not null default 0,
    reply_to_bot_count bigint not null default 0,
    top_level_message_count bigint not null default 0,
    reply_to_comment_count bigint not null default 0,
    only_replies_or_comments boolean not null default false,
    only_channel_post_comments boolean not null default false,
    message_count_24h bigint not null default 0,
    link_count_24h bigint not null default 0,
    burst_messages_per_min double precision,
    first_message_text text,
    first_message_len integer,
    last_message_text text,
    last_message_len integer,
    normalized_message_count bigint not null default 0,
    distinct_normalized_message_count bigint not null default 0,
    duplicate_normalized_message_count bigint not null default 0,
    max_normalized_message_reuse_count bigint not null default 0,
    max_pairwise_message_similarity double precision,
    avg_message_len double precision,
    repetitive_message_pattern boolean not null default false,

    telegram_user_id_bucket text,
    telegram_user_id_rank_ratio double precision,
    telegram_user_id_is_recent boolean not null default false,

    username text,
    username_len integer,
    username_has_digits boolean not null default false,
    username_digit_count integer not null default 0,
    username_has_random_suffix boolean not null default false,
    username_pattern text,
    first_name text,
    last_name text,
    display_name text,
    first_name_feminine_pattern boolean not null default false,
    display_name_reuse_count bigint not null default 0,
    display_name_reuse_spammer_count bigint not null default 0,
    display_name_reused_by_spammers boolean not null default false,
    is_bot boolean not null default false,
    is_premium boolean,
    language_code text,
    bio text,
    bio_len integer,

    has_profile_photo boolean not null default false,
    profile_photo_count integer,
    profile_photo_reuse_count bigint not null default 0,
    profile_photo_file_unique_id text,
    profile_photo_dc_id integer,
    profile_photo_dc_source text,
    profile_photo_width integer,
    profile_photo_height integer,
    has_emoji_status boolean not null default false,
    profile_accent_color_id smallint,

    personal_channel_chat_id bigint,
    personal_channel_title text,
    personal_channel_username text,
    personal_channel_message_count integer,
    personal_channel_last_message_id integer,
    personal_channel_last_message_at timestamptz,
    personal_channel_last_text text,
    personal_channel_has_adult_links boolean not null default false,
    personal_channel_has_invite_link boolean not null default false,
    personal_channel_has_external_link boolean not null default false,
    personal_channel_title_len integer,
    personal_channel_last_text_len integer,
    personal_channel_refreshed_at timestamptz,
    personal_channel_fetch_error text,

    member_status text,
    member_is_present boolean,
    member_is_admin boolean,
    join_event_seen boolean not null default false,
    invite_link text,
    via_chat_folder_invite_link boolean not null default false,

    risk_score integer not null default 0,
    risk_level text not null default 'low',
    primary_risk_class text,
    risk_class_scores jsonb not null default '{}'::jsonb,
    risk_labels jsonb not null default '[]'::jsonb,
    risk_reasons jsonb not null default '[]'::jsonb,
    raw_features jsonb not null default '{}'::jsonb,

    primary key (chat_id, telegram_user_id)
);

create index if not exists telegram_new_user_profile_audits_analyzed_idx
    on telegram_new_user_profile_audits (chat_id, analyzed_at desc);

create index if not exists telegram_new_user_profile_audits_risk_idx
    on telegram_new_user_profile_audits (chat_id, risk_score desc, analyzed_at desc);

create index if not exists telegram_new_user_profile_audits_class_idx
    on telegram_new_user_profile_audits (chat_id, primary_risk_class, risk_score desc)
    where primary_risk_class is not null;

create index if not exists telegram_new_user_profile_audits_recent_id_idx
    on telegram_new_user_profile_audits (chat_id, telegram_user_id_is_recent, risk_score desc)
    where telegram_user_id_is_recent;

create index if not exists telegram_new_user_profile_audits_display_name_idx
    on telegram_new_user_profile_audits (chat_id, lower(display_name))
    where display_name is not null;

create index if not exists telegram_new_user_profile_audits_channel_idx
    on telegram_new_user_profile_audits (personal_channel_chat_id)
    where personal_channel_chat_id is not null;
