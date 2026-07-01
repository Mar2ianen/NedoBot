alter table telegram_chat_users
    add column if not exists spam_profile_labels jsonb not null default '[]'::jsonb,
    add column if not exists spam_profile_note text;
