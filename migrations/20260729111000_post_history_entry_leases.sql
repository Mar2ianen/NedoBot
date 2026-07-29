alter table public.post_history_entries
    add column if not exists lease_expires_at timestamptz;

update public.post_history_entries
set lease_expires_at = coalesce(
    processing_started_at + interval '5 minutes',
    now() - interval '1 microsecond'
)
where status = 'processing'
  and lease_expires_at is null;

create index if not exists post_history_entries_processing_lease_idx
    on public.post_history_entries (lease_expires_at, id)
    where status = 'processing';
