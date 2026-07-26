alter table public.post_comment_jobs
    add column if not exists attempts integer not null default 0,
    add column if not exists next_attempt_at timestamptz not null default now(),
    add column if not exists processing_started_at timestamptz,
    add column if not exists lease_expires_at timestamptz,
    add column if not exists error_kind text,
    add column if not exists image_file_id text,
    add column if not exists image_file_unique_id text;

alter table public.post_comment_jobs
    add constraint post_comment_jobs_attempts_nonnegative check (attempts >= 0);

create index if not exists post_comment_jobs_ready_idx
    on public.post_comment_jobs (next_attempt_at, id)
    where status in ('pending', 'retry_wait');

create index if not exists post_comment_jobs_processing_lease_idx
    on public.post_comment_jobs (lease_expires_at)
    where status = 'processing';
