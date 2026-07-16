alter table avatar_analysis_jobs
    add column if not exists features_json jsonb not null default '{}'::jsonb;
