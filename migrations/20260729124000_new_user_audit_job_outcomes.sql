alter table new_user_audit_jobs
    add column if not exists avatar_file_id text,
    add column if not exists avatar_file_unique_id text,
    add column if not exists assessment_json jsonb,
    add column if not exists provider text,
    add column if not exists model text,
    add column if not exists completed_at timestamptz;
