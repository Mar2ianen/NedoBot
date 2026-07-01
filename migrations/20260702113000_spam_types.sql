alter table telegram_messages
    add column if not exists spam_type text;

alter table telegram_chat_users
    add column if not exists spam_type text,
    add column if not exists spam_types jsonb not null default '{}'::jsonb;

create index if not exists telegram_messages_spam_type_idx
    on telegram_messages (spam_type, spam_marked_at desc)
    where spam_type is not null;

create index if not exists telegram_chat_users_spam_type_idx
    on telegram_chat_users (chat_id, spam_type, spam_score desc)
    where spam_type is not null;
