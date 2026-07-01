alter table telegram_messages
    add column if not exists edited_at timestamptz,
    add column if not exists edit_count integer not null default 0,
    add column if not exists deleted_by_bot_at timestamptz,
    add column if not exists deleted_by_bot_reason text,
    add column if not exists deleted_by_bot_actor_id bigint;

create table if not exists telegram_message_edits (
    id bigserial primary key,
    chat_id bigint not null,
    message_id integer not null,
    user_id bigint,
    old_text text,
    new_text text,
    old_raw_json jsonb,
    new_raw_json jsonb not null default '{}'::jsonb,
    edited_at timestamptz not null,
    observed_at timestamptz not null default now()
);

create index if not exists telegram_message_edits_message_idx
    on telegram_message_edits (chat_id, message_id, edited_at desc);

create index if not exists telegram_messages_deleted_by_bot_idx
    on telegram_messages (deleted_by_bot_at)
    where deleted_by_bot_at is not null;
