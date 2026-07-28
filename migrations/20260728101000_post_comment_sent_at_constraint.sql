-- Preserve historical sent jobs before enforcing the invariant.
update public.post_comment_jobs
set sent_at = updated_at
where status = 'sent' and sent_at is null;

alter table public.post_comment_jobs
    add constraint post_comment_jobs_sent_requires_sent_at
    check (status <> 'sent' or sent_at is not null);
