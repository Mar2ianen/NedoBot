alter table telegram_user_notes
    add column if not exists source_message_ids jsonb not null default '[]'::jsonb;
