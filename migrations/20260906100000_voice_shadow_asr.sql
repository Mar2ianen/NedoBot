alter table public.voice_transcription_jobs
    add column if not exists asr_alternatives_json jsonb;
