create extension if not exists pg_trgm;

create index if not exists telegram_messages_ask_trgm_idx
    on public.telegram_messages using gin (lower(text) gin_trgm_ops)
    where text is not null and deleted_by_bot_at is null;
