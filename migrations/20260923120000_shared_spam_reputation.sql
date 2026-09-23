create table if not exists shared_spam_reputation (
    telegram_user_id bigint not null,
    source_instance_id text not null,
    synced_at timestamptz not null default now(),
    primary key (telegram_user_id, source_instance_id)
);

create index if not exists shared_spam_reputation_source_idx
    on shared_spam_reputation (source_instance_id, telegram_user_id);
