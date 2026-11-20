-- Каналы — отдельные цели, не пользовательские identities и не spam account labels.
create table manual_sender_chat_operations (
    id bigserial primary key,
    chat_id bigint not null,
    command_message_id integer not null,
    sender_chat_id bigint not null check (sender_chat_id < 0 and sender_chat_id <> chat_id),
    actor_user_id bigint not null check (actor_user_id > 0),
    action text not null check (action in ('ban', 'unban')),
    reason text,
    status text not null check (status in ('started', 'succeeded', 'unknown')),
    created_at timestamptz not null default now(),
    completed_at timestamptz,
    unique(chat_id, command_message_id)
);
create index on manual_sender_chat_operations(chat_id,sender_chat_id);
revoke all on manual_sender_chat_operations from public;
do $$ begin
    if exists(select 1 from pg_roles where rolname='nedobot_mcp_ro') then
        revoke all on manual_sender_chat_operations from nedobot_mcp_ro;
    end if;
end $$;
