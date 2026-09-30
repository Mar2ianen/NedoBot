alter table manual_moderation_batches
    add column request_json jsonb not null default '{}'::jsonb,
    add column status text not null default 'completed'
        check (status in ('accepted', 'prepared', 'running', 'completed', 'unknown')),
    add column result_text text,
    add column processing_lease_expires_at timestamptz,
    add column updated_at timestamptz not null default now();

update manual_moderation_batches
set result_text = 'Команда обработана до включения сохранения batch-результатов.'
where result_text is null;

create index manual_moderation_batches_recovery_idx
    on manual_moderation_batches (status, processing_lease_expires_at)
    where status in ('accepted', 'prepared', 'running', 'unknown');

create unique index manual_moderation_one_warning_per_batch_target_idx
    on manual_moderation_actions (batch_id, target_user_id)
    where action = 'warn';

create unique index manual_moderation_one_restriction_per_batch_target_idx
    on manual_moderation_actions (batch_id, target_user_id, action)
    where action in ('mute', 'ban', 'auto_mute');

drop index if exists manual_moderation_one_processing_action_idx;

create unique index manual_moderation_one_inflight_action_idx
    on manual_moderation_actions (chat_id, target_user_id)
    where status in ('pending', 'processing');
