create table if not exists post_memory_notes (
    id bigserial primary key,
    source_channel_id bigint not null,
    source_message_id integer not null,
    title text not null,
    summary text not null,
    cautions text not null default '',
    keywords text[] not null default '{}',
    raw_note text not null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (source_channel_id, source_message_id)
);

create index if not exists post_memory_notes_keywords_idx
    on post_memory_notes using gin (keywords);

create index if not exists post_memory_notes_created_at_idx
    on post_memory_notes (created_at desc);
