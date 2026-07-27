alter table public.post_comment_jobs
    add column if not exists sent_at timestamptz;

update public.post_comment_jobs
set sent_at = updated_at
where status = 'sent' and sent_at is null;

create index if not exists post_comment_jobs_sent_at_idx
    on public.post_comment_jobs (discussion_chat_id, sent_at)
    where status = 'sent';
