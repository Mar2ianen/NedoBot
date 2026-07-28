alter table spam_review_requests
    add column if not exists notification_status text not null default 'pending'
        check (notification_status in ('pending', 'processing', 'retry_wait', 'sent')),
    add column if not exists notification_attempts integer not null default 0,
    add column if not exists notification_next_attempt_at timestamptz not null default now(),
    add column if not exists notification_processing_started_at timestamptz,
    add column if not exists notification_lease_expires_at timestamptz,
    add column if not exists notification_error_kind text,
    add column if not exists notification_message_id integer;

create index if not exists spam_review_requests_notification_ready_idx
    on spam_review_requests (notification_next_attempt_at, id)
    where status = 'pending' and notification_status in ('pending', 'retry_wait');
