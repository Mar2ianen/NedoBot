alter table telegram_messages
    add column if not exists updated_at timestamptz not null default now();

create index if not exists telegram_messages_updated_at_idx
    on telegram_messages (updated_at desc);
