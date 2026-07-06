create table if not exists search_runs (
    id bigserial primary key,
    post_comment_job_id bigint not null references post_comment_jobs(id) on delete cascade,
    status text not null,
    skipped_reason text,
    latency_ms bigint not null default 0,
    queries jsonb not null default '[]'::jsonb,
    results jsonb not null default '[]'::jsonb,
    created_at timestamptz not null default now()
);

create index if not exists search_runs_post_comment_job_id_idx
    on search_runs (post_comment_job_id);

create index if not exists search_runs_created_at_idx
    on search_runs (created_at desc);

create index if not exists search_runs_status_idx
    on search_runs (status);
