create table if not exists spam_review_requests (
    id bigserial primary key,
    chat_id bigint not null,
    telegram_user_id bigint not null,
    risk_score integer not null,
    risk_signals jsonb not null default '[]'::jsonb,
    status text not null default 'pending'
        check (status in ('pending', 'confirmed_spam', 'confirmed_not_spam')),
    notified_at timestamptz not null default now(),
    reviewed_at timestamptz,
    reviewed_by_user_id bigint,
    unique (chat_id, telegram_user_id)
);

create index if not exists spam_review_requests_pending_idx
    on spam_review_requests (notified_at desc) where status = 'pending';
