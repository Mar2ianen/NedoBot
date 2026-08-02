alter table public.voice_transcription_jobs
    add column if not exists attempts integer not null default 0,
    add column if not exists next_attempt_at timestamptz not null default now(),
    add column if not exists processing_started_at timestamptz,
    add column if not exists lease_expires_at timestamptz,
    add column if not exists error_kind text,
    add column if not exists progress_message_id integer;

alter table public.voice_transcription_jobs
    drop constraint if exists voice_transcription_jobs_status_check;

alter table public.voice_transcription_jobs
    add constraint voice_transcription_jobs_status_check
    check (status in (
        'pending', 'retry_wait', 'processing', 'downloading', 'transcribing',
        'cleaning', 'sent', 'failed', 'skipped'
    ));

create index if not exists voice_transcription_jobs_claim_idx
    on public.voice_transcription_jobs (status, next_attempt_at, lease_expires_at, id);
