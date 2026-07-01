create table if not exists voice_transcription_jobs (
    id bigserial primary key,
    chat_id bigint not null,
    message_id integer not null,
    user_id bigint,
    file_id text not null,
    file_unique_id text,
    media_kind text not null,
    duration_sec integer,
    file_size bigint,
    mime_type text,
    status text not null default 'pending',
    error text,
    asr_provider text,
    asr_model text,
    asr_request_id text,
    cleanup_provider text,
    cleanup_model text,
    raw_transcript text,
    cleaned_text text,
    render_mode text,
    chapters_json jsonb,
    segments_json jsonb,
    raw_asr_json jsonb,
    final_html text,
    full_text_file_id text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (chat_id, message_id),
    check (status in ('pending', 'downloading', 'transcribing', 'cleaning', 'sent', 'failed', 'skipped'))
);

create index if not exists voice_transcription_jobs_status_idx
    on voice_transcription_jobs (status);

create index if not exists voice_transcription_jobs_created_at_idx
    on voice_transcription_jobs (created_at desc);
