alter table public.post_comment_jobs
    add column if not exists lease_reclaim_count integer not null default 0,
    add constraint post_comment_jobs_lease_reclaim_count_nonnegative
        check (lease_reclaim_count >= 0);

alter table public.telegram_message_embeddings
    add column if not exists lease_reclaim_count integer not null default 0,
    add constraint telegram_message_embeddings_lease_reclaim_count_nonnegative
        check (lease_reclaim_count >= 0);

alter table public.post_history_entries
    add column if not exists lease_reclaim_count integer not null default 0,
    add constraint post_history_entries_lease_reclaim_count_nonnegative
        check (lease_reclaim_count >= 0);

alter table public.spam_review_requests
    add column if not exists notification_lease_reclaim_count integer not null default 0,
    add constraint spam_review_requests_notification_lease_reclaim_count_nonnegative
        check (notification_lease_reclaim_count >= 0);
