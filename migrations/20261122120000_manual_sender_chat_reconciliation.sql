-- Административное подтверждение исхода ручной операции с каналом.
-- Новый статус failed означает проверенный отказ/отсутствие эффекта, а не
-- автоматический retry. Reconciliation не вызывает Telegram API.
alter table manual_sender_chat_operations
    drop constraint if exists manual_sender_chat_operations_status_check;
alter table manual_sender_chat_operations
    add constraint manual_sender_chat_operations_status_check
        check (status in ('started', 'succeeded', 'unknown', 'failed'));
alter table manual_sender_chat_operations
    add column reconciled_at timestamptz,
    add column reconciled_by bigint check (reconciled_by > 0),
    add column resolution text check (resolution in ('applied', 'not_applied')),
    add column reconciliation_note text;
alter table manual_sender_chat_operations
    add constraint manual_sender_chat_operations_reconciliation_check
        check (
            (reconciled_at is null) = (reconciled_by is null)
            and (reconciled_at is null) = (resolution is null)
            and (reconciled_at is not null or reconciliation_note is null)
            and (
                reconciled_at is null
                or (status = 'succeeded' and resolution = 'applied')
                or (status = 'failed' and resolution = 'not_applied')
            )
        );
create index manual_sender_chat_operations_pending_idx
    on manual_sender_chat_operations(chat_id, id)
    where status in ('started', 'unknown');
