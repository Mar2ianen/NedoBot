create table if not exists telegram_users (
    id bigserial primary key,
    telegram_user_id bigint not null unique,
    username text,
    first_name text,
    last_name text,
    is_admin boolean not null default false,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table if not exists telegram_chats (
    id bigserial primary key,
    telegram_chat_id bigint not null unique,
    title text,
    kind text not null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table if not exists bot_settings (
    key text primary key,
    value text not null,
    updated_at timestamptz not null default now()
);

create table if not exists admin_events (
    id bigserial primary key,
    telegram_user_id bigint,
    telegram_chat_id bigint,
    event_type text not null,
    payload jsonb not null default '{}'::jsonb,
    created_at timestamptz not null default now()
);
