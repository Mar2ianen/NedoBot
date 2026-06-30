alter table telegram_messages
    add column if not exists reply_to_message_id integer,
    add column if not exists reply_to_user_id bigint,
    add column if not exists sender_chat_id bigint,
    add column if not exists via_bot_id bigint,
    add column if not exists has_photo boolean not null default false,
    add column if not exists has_video boolean not null default false,
    add column if not exists has_document boolean not null default false,
    add column if not exists has_audio boolean not null default false,
    add column if not exists has_voice boolean not null default false,
    add column if not exists has_sticker boolean not null default false,
    add column if not exists has_animation boolean not null default false,
    add column if not exists has_links boolean not null default false;

create index if not exists telegram_messages_created_at_idx
    on telegram_messages (created_at);

create index if not exists telegram_messages_user_created_idx
    on telegram_messages (user_id, created_at);

create index if not exists telegram_messages_reply_idx
    on telegram_messages (chat_id, reply_to_message_id);

create table if not exists telegram_user_profiles (
    telegram_user_id bigint primary key,
    username text,
    first_name text,
    last_name text,
    is_bot boolean not null default false,
    is_premium boolean,
    language_code text,
    last_seen_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table if not exists telegram_chat_member_snapshots (
    chat_id bigint not null,
    telegram_user_id bigint not null,
    status text not null,
    is_admin boolean not null default false,
    is_present boolean not null default false,
    raw_json jsonb not null default '{}'::jsonb,
    observed_at timestamptz not null default now(),
    primary key (chat_id, telegram_user_id)
);

create table if not exists telegram_message_reactions (
    id bigserial primary key,
    chat_id bigint not null,
    message_id integer not null,
    user_id bigint,
    actor_chat_id bigint,
    old_reactions jsonb not null default '[]'::jsonb,
    new_reactions jsonb not null default '[]'::jsonb,
    raw_json jsonb not null default '{}'::jsonb,
    event_at timestamptz not null,
    created_at timestamptz not null default now()
);

create index if not exists telegram_message_reactions_message_idx
    on telegram_message_reactions (chat_id, message_id, event_at desc);

create index if not exists telegram_message_reactions_user_idx
    on telegram_message_reactions (user_id, event_at desc);

create table if not exists telegram_message_reaction_counts (
    chat_id bigint not null,
    message_id integer not null,
    reactions jsonb not null default '[]'::jsonb,
    total_count integer not null default 0,
    raw_json jsonb not null default '{}'::jsonb,
    event_at timestamptz not null,
    updated_at timestamptz not null default now(),
    primary key (chat_id, message_id)
);

create table if not exists telegram_chat_member_events (
    id bigserial primary key,
    chat_id bigint not null,
    telegram_user_id bigint not null,
    actor_user_id bigint,
    old_status text not null,
    new_status text not null,
    invite_link text,
    via_chat_folder_invite_link boolean not null default false,
    raw_json jsonb not null default '{}'::jsonb,
    event_at timestamptz not null,
    created_at timestamptz not null default now()
);

create index if not exists telegram_chat_member_events_user_idx
    on telegram_chat_member_events (telegram_user_id, event_at desc);

create index if not exists telegram_chat_member_events_chat_idx
    on telegram_chat_member_events (chat_id, event_at desc);
