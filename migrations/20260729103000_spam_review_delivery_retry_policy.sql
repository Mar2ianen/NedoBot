alter table spam_review_requests
    add column if not exists notification_consecutive_failures integer not null default 0;

drop index if exists spam_review_requests_notification_ready_idx;

create index spam_review_requests_notification_ready_idx
    on spam_review_requests (notification_next_attempt_at, id)
    where status = 'pending'
      and risk_score >= 70
      and notification_status in ('pending', 'retry_wait');
