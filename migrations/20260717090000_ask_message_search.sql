create index if not exists telegram_messages_ask_russian_fts_idx
    on telegram_messages using gin (to_tsvector('russian', coalesce(text, '')))
    where text is not null and deleted_by_bot_at is null;

create index if not exists telegram_messages_ask_simple_fts_idx
    on telegram_messages using gin (to_tsvector('simple', coalesce(text, '')))
    where text is not null and deleted_by_bot_at is null;
