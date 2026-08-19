alter table public.voice_transcription_jobs
    drop constraint if exists voice_transcription_jobs_status_check;

alter table public.voice_transcription_jobs
    add constraint voice_transcription_jobs_status_check
    check (status in (
        'pending', 'retry_wait', 'processing', 'downloading', 'transcribing',
        'cleaning', 'delivering', 'delivery_unknown', 'sent', 'failed', 'skipped'
    ));

create index if not exists voice_transcription_jobs_delivery_lease_idx
    on public.voice_transcription_jobs (lease_expires_at, id)
    where status = 'delivering';
