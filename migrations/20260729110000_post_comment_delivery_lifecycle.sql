-- A legacy processing row may have crossed the Telegram delivery boundary before
-- this schema could record it. Preserve it for explicit reconciliation rather
-- than risking a duplicate automatic comment.
alter table public.post_comment_jobs
    add column if not exists sending_started_at timestamptz;

update public.post_comment_jobs
set status = 'delivery_unknown',
    lease_expires_at = null,
    updated_at = now()
where status = 'processing';

alter table public.post_comment_jobs
    add constraint post_comment_jobs_status_check
        check (status in (
            'pending', 'retry_wait', 'processing', 'sending', 'sent', 'failed',
            'delivery_unknown'
        ));

create index if not exists post_comment_jobs_sending_lease_idx
    on public.post_comment_jobs (lease_expires_at)
    where status = 'sending';

create index if not exists post_comment_jobs_delivery_unknown_idx
    on public.post_comment_jobs (updated_at, id)
    where status = 'delivery_unknown';

-- Do not silently discard audit records in production. Operators must resolve
-- historical duplicates before this idempotency constraint can be installed.
do $$
begin
    if exists (
        select 1
        from public.llm_generations
        where post_comment_job_id is not null
        group by post_comment_job_id
        having count(*) > 1
    ) then
        raise exception
            'cannot add llm_generations(post_comment_job_id) uniqueness: existing duplicate post_comment_job_id rows require manual reconciliation';
    end if;
end $$;

create unique index llm_generations_post_comment_job_id_unique
    on public.llm_generations (post_comment_job_id)
    where post_comment_job_id is not null;
