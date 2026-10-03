-- Локальное зеркало LOLS banlist (https://api.lols.bot/lists, spammers-full,
-- почасовое обновление). Только user_id > 0: чаты/каналы из списка здесь не
-- нужны. Синхронизирует bin sync_lols_banlist через temp swap.
create table if not exists lols_spam_users (
    telegram_user_id bigint primary key,
    first_seen_at timestamptz not null default now(),
    last_seen_at timestamptz not null default now()
);

create index if not exists lols_spam_users_seen_idx
    on lols_spam_users (last_seen_at desc);
