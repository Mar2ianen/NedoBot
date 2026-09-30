create table if not exists manual_moderation_batches (
    id bigserial primary key,
    chat_id bigint not null,
    actor_user_id bigint not null,
    source_message_id integer not null,
    command text not null,
    created_at timestamptz not null default now(),
    unique (chat_id, source_message_id)
);

create table if not exists manual_moderation_actions (
    id bigserial primary key,
    batch_id bigint not null references manual_moderation_batches(id),
    chat_id bigint not null,
    target_user_id bigint not null,
    actor_user_id bigint not null,
    action text not null check (action in ('mute', 'ban', 'warn', 'auto_mute')),
    reason text,
    status text not null check (
        status in ('pending', 'processing', 'applied', 'failed', 'unknown', 'revoked', 'superseded', 'expired')
    ),
    created_at timestamptz not null default now(),
    expires_at timestamptz,
    processing_lease_expires_at timestamptz,
    supersedes_action_id bigint references manual_moderation_actions(id),
    automatic boolean not null default false,
    check (expires_at is null or expires_at > created_at)
);

create index if not exists manual_moderation_actions_target_idx
    on manual_moderation_actions (chat_id, target_user_id, created_at desc);

create index if not exists manual_moderation_active_warns_idx
    on manual_moderation_actions (chat_id, target_user_id, expires_at)
    where action = 'warn' and status = 'applied';

create unique index if not exists manual_moderation_one_processing_action_idx
    on manual_moderation_actions (chat_id, target_user_id)
    where status = 'processing';

create unique index if not exists manual_moderation_one_applied_restriction_idx
    on manual_moderation_actions (chat_id, target_user_id)
    where status = 'applied' and action in ('mute', 'ban', 'auto_mute');

create table if not exists manual_moderation_events (
    id bigserial primary key,
    batch_id bigint not null references manual_moderation_batches(id),
    action_id bigint references manual_moderation_actions(id),
    chat_id bigint not null,
    target_user_id bigint,
    actor_user_id bigint not null,
    event text not null check (
        event in ('requested', 'applied', 'failed', 'unknown', 'revoked', 'superseded', 'undo_skipped')
    ),
    reason text,
    details text,
    created_at timestamptz not null default now()
);

create index if not exists manual_moderation_events_target_idx
    on manual_moderation_events (chat_id, target_user_id, created_at desc);
