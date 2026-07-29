-- Reconciliation is intentionally explicit: an ambiguous Telegram send must never return
-- to the automatic worker. Only the operator CLI can claim it after acknowledgement.
alter table public.post_comment_jobs
    add column if not exists operator_retry_only boolean not null default false;

create index if not exists post_comment_jobs_operator_retry_processing_idx
    on public.post_comment_jobs (lease_expires_at)
    where status = 'processing' and operator_retry_only;

create table if not exists public.post_comment_job_operator_audit (
    id bigserial primary key,
    post_comment_job_id bigint not null references public.post_comment_jobs(id) on delete restrict,
    action text not null check (action in ('mark_delivered', 'mark_failed', 'retry')),
    actor text not null check (char_length(actor) between 1 and 128),
    reason text not null check (char_length(reason) between 1 and 1000),
    previous_status text not null,
    resulting_status text not null,
    created_at timestamptz not null default now()
);

create index if not exists post_comment_job_operator_audit_job_created_idx
    on public.post_comment_job_operator_audit (post_comment_job_id, created_at desc);
