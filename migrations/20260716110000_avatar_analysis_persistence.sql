create table if not exists avatar_image_analyses (
    profile_photo_file_unique_id text not null,
    prompt_version text not null,
    provider text not null,
    model text not null,
    input_hash text not null,
    observation_json jsonb not null,
    response_json jsonb not null,
    analyzed_at timestamptz not null default now(),
    primary key (profile_photo_file_unique_id, prompt_version)
);

create table if not exists avatar_profile_assessments (
    telegram_user_id bigint not null references telegram_user_profiles (telegram_user_id) on delete cascade,
    profile_photo_file_unique_id text not null,
    features_snapshot_hash text not null,
    prompt_version text not null,
    provider text not null,
    model text not null,
    input_hash text not null,
    assessment_json jsonb not null,
    response_json jsonb not null,
    analyzed_at timestamptz not null default now(),
    primary key (telegram_user_id, profile_photo_file_unique_id, features_snapshot_hash, prompt_version)
);

create index if not exists avatar_profile_assessments_user_idx
    on avatar_profile_assessments (telegram_user_id, analyzed_at desc);

create table if not exists avatar_analysis_jobs (
    id bigserial primary key,
    telegram_user_id bigint not null references telegram_user_profiles (telegram_user_id) on delete cascade,
    profile_photo_file_id text not null,
    profile_photo_file_unique_id text not null,
    features_snapshot_hash text not null,
    prompt_version text not null,
    status text not null default 'pending'
        check (status in ('pending', 'processing', 'retry_wait', 'succeeded', 'failed')),
    attempts integer not null default 0 check (attempts >= 0),
    next_attempt_at timestamptz not null default now(),
    processing_started_at timestamptz,
    lease_expires_at timestamptz,
    provider text,
    model text,
    error_kind text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (telegram_user_id, profile_photo_file_unique_id, features_snapshot_hash, prompt_version)
);

create index if not exists avatar_analysis_jobs_ready_idx
    on avatar_analysis_jobs (next_attempt_at, id)
    where status in ('pending', 'retry_wait');

create index if not exists avatar_analysis_jobs_lease_idx
    on avatar_analysis_jobs (lease_expires_at)
    where status = 'processing';
