alter table telegram_messages
    add column if not exists spam_marked_at timestamptz,
    add column if not exists spam_reason text,
    add column if not exists spam_source text,
    add column if not exists spam_marked_by_user_id bigint;

alter table telegram_chat_users
    add column if not exists is_spammer boolean not null default false,
    add column if not exists spam_score integer not null default 0,
    add column if not exists spam_message_count bigint not null default 0,
    add column if not exists spam_last_marked_at timestamptz,
    add column if not exists spam_reason text;

create index if not exists telegram_messages_spam_marked_idx
    on telegram_messages (spam_marked_at desc)
    where spam_marked_at is not null;

create index if not exists telegram_chat_users_spammer_idx
    on telegram_chat_users (chat_id, is_spammer, spam_score desc)
    where is_spammer;
