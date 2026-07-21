alter table public.post_history_entries
    drop constraint if exists post_history_entries_status_check;

alter table public.post_history_entries
    add constraint post_history_entries_status_check
        check (status in ('pending', 'processing', 'ready', 'ignored', 'retry', 'failed'));
