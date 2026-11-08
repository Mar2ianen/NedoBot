-- Persistent /ask exchanges. These are untrusted conversational context, not
-- verified facts, and are deliberately absent from every mcp_public view.
create table if not exists ask_conversation_memory (
    instance_id text not null check (length(btrim(instance_id)) > 0),
    profile_key text not null check (length(btrim(profile_key)) > 0),
    chat_id bigint not null check (chat_id <> 0),
    -- A verified Telegram reply-chain root; zero is the no-reply bucket.
    thread_root_message_id integer not null check (thread_root_message_id >= 0),
    participant_user_id bigint not null check (participant_user_id > 0),
    command_message_id integer not null check (command_message_id > 0),
    reply_to_message_id integer,
    user_text text not null check (length(user_text) > 0),
    assistant_text text not null check (length(assistant_text) > 0),
    source_message_ids jsonb not null default '[]'::jsonb
        check (jsonb_typeof(source_message_ids) = 'array'),
    source_urls jsonb not null default '[]'::jsonb
        check (jsonb_typeof(source_urls) = 'array'),
    created_at timestamptz not null default now(),
    expires_at timestamptz not null,
    primary key (
        instance_id, profile_key, chat_id, thread_root_message_id,
        participant_user_id, command_message_id
    )
);

create index if not exists ask_conversation_memory_recent_idx
    on ask_conversation_memory (
        instance_id, profile_key, chat_id, thread_root_message_id, participant_user_id,
        created_at desc, command_message_id desc
    );

create index if not exists ask_conversation_memory_expiry_idx
    on ask_conversation_memory (expires_at);

comment on table ask_conversation_memory is
    'Bounded private /ask conversational context; never publish through MCP views or treat as verified facts';

create table if not exists ask_conversation_memory_epochs (
    instance_id text not null,
    profile_key text not null,
    chat_id bigint not null,
    thread_root_message_id integer not null,
    participant_user_id bigint not null,
    epoch bigint not null default 0 check (epoch >= 0),
    updated_at timestamptz not null default now(),
    primary key (instance_id, profile_key, chat_id, thread_root_message_id, participant_user_id)
);

create index if not exists ask_conversation_memory_epochs_updated_idx
    on ask_conversation_memory_epochs (updated_at);

comment on table ask_conversation_memory_epochs is
    'Forget tombstones prevent in-flight /ask responses from restoring cleared conversation memory';

revoke all on table ask_conversation_memory from public;
revoke all on table ask_conversation_memory_epochs from public;
do $$
begin
    if exists (select 1 from pg_roles where rolname = 'nedobot_mcp_ro') then
        revoke all on table ask_conversation_memory from nedobot_mcp_ro;
        revoke all on table ask_conversation_memory_epochs from nedobot_mcp_ro;
    end if;
end
$$;
