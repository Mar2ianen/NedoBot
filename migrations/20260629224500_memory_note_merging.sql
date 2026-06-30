alter table post_memory_notes
    add column if not exists merged_source_posts integer not null default 1,
    add column if not exists last_source_channel_id bigint,
    add column if not exists last_source_message_id integer;

update post_memory_notes
set last_source_channel_id = coalesce(last_source_channel_id, source_channel_id),
    last_source_message_id = coalesce(last_source_message_id, source_message_id);
