create table manual_moderation_operations (
    id bigserial primary key,
    batch_id bigint not null references manual_moderation_batches(id) on delete cascade,
    action_id bigint not null references manual_moderation_actions(id) on delete cascade,
    chat_id bigint not null,
    target_user_id bigint not null,
    actor_user_id bigint not null,
    operation_kind text not null check (
        operation_kind in ('apply', 'revoke', 'undo', 'restore', 'legacy')
    ),
    status text not null check (
        status in ('pending', 'processing', 'succeeded', 'failed', 'unknown')
    ),
    lease_expires_at timestamptz,
    details text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (batch_id, action_id, operation_kind)
);

create unique index manual_moderation_one_inflight_operation_idx
    on manual_moderation_operations (chat_id, target_user_id)
    where status in ('pending', 'processing');

create index manual_moderation_operations_recovery_idx
    on manual_moderation_operations (batch_id, status, lease_expires_at)
    where status in ('pending', 'processing', 'unknown');

insert into manual_moderation_operations
    (batch_id, action_id, chat_id, target_user_id, actor_user_id, operation_kind, status)
select batch_id, id, chat_id, target_user_id, actor_user_id, 'apply', 'pending'
from manual_moderation_actions
where status = 'pending'
on conflict (batch_id, action_id, operation_kind) do nothing;

with recovered as (
    update manual_moderation_actions
    set status = 'unknown', processing_lease_expires_at = null
    where status = 'processing'
    returning id, batch_id, chat_id, target_user_id, actor_user_id
), operations as (
    insert into manual_moderation_operations
        (batch_id, action_id, chat_id, target_user_id, actor_user_id, operation_kind, status,
         details)
    select batch_id, id, chat_id, target_user_id, actor_user_id, 'legacy', 'unknown',
           'legacy processing action recovered during operation-intent migration'
    from recovered
    on conflict (batch_id, action_id, operation_kind) do nothing
    returning action_id
)
insert into manual_moderation_events
    (batch_id, action_id, chat_id, target_user_id, actor_user_id, event, details)
select recovered.batch_id, recovered.id, recovered.chat_id, recovered.target_user_id,
       recovered.actor_user_id, 'unknown',
       'legacy processing action recovered during operation-intent migration'
from recovered
join operations on operations.action_id = recovered.id;
