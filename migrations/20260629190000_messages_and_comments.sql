create table if not exists telegram_messages (
    id bigserial primary key,
    chat_id bigint not null,
    message_id integer not null,
    user_id bigint,
    source_channel_id bigint,
    source_message_id integer,
    is_automatic_forward boolean not null default false,
    text text,
    raw_json jsonb not null default '{}'::jsonb,
    created_at timestamptz not null default now(),
    unique (chat_id, message_id)
);

create table if not exists post_comment_jobs (
    id bigserial primary key,
    discussion_chat_id bigint not null,
    discussion_message_id integer not null,
    source_channel_id bigint not null,
    source_message_id integer not null,
    cleaned_post_text text not null,
    status text not null default 'pending',
    error text,
    bot_comment_message_id integer,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (discussion_chat_id, discussion_message_id),
    unique (source_channel_id, source_message_id)
);

create table if not exists llm_generations (
    id bigserial primary key,
    post_comment_job_id bigint references post_comment_jobs(id) on delete cascade,
    provider text not null,
    model text not null,
    prompt text not null,
    image_used boolean not null default false,
    response text,
    final_html text,
    created_at timestamptz not null default now()
);
