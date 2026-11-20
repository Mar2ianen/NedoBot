-- Durable profile-refresh / unified-audit intent.
-- The bounded in-memory queue only accelerates delivery; every ingested human
-- message also records a per-(instance, chat, user) revision here so a full or
-- closed queue, a crash, or a restart cannot lose the eventual audit trigger.
create table if not exists public.telegram_profile_refresh_intents (
    instance_id text not null,
    chat_id bigint not null,
    telegram_user_id bigint not null,
    revision bigint not null default 1,
    attempts integer not null default 0,
    next_attempt_at timestamptz not null default now(),
    completed_revision bigint,
    failed_at timestamptz,
    last_error text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (instance_id, chat_id, telegram_user_id)
);

create index if not exists telegram_profile_refresh_intents_due_idx
    on public.telegram_profile_refresh_intents (next_attempt_at, instance_id, chat_id, telegram_user_id)
    where completed_revision is null or completed_revision < revision;
