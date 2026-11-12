-- Изолированная shadow-оценка. Не входит в MCP projections или moderation evidence.
create table moderation_text_observations (
    instance_id text not null,
    chat_id bigint not null,
    message_id integer not null,
    user_id bigint not null,
    source_hash text not null,
    eligible boolean not null,
    reason text not null,
    local_candidate boolean not null,
    features jsonb not null,
    observed_at timestamptz not null default now(),
    primary key (instance_id, chat_id, message_id)
);
create table moderation_decision_jobs (
    id bigserial primary key,
    instance_id text not null,
    chat_id bigint not null,
    message_id integer not null,
    user_id bigint not null,
    snapshot_hash text not null,
    source_hash text not null,
    source_edited_at timestamptz,
    label_event_id bigint not null,
    question_version text not null,
    normalization_version text not null,
    policy_version text not null,
    model text not null,
    input_json jsonb not null,
    avatar_file_id text,
    avatar_unique_id text,
    status text not null default 'pending'
        check (status in ('pending','processing','retry_wait','succeeded','failed','stale')),
    attempts integer not null default 0 check (attempts >= 0),
    fence bigint not null default 0,
    next_attempt_at timestamptz not null default now(),
    lease_expires_at timestamptz,
    error_kind text,
    assessment_json jsonb,
    shadow_verdict text check (shadow_verdict in ('review_candidate','no_signal','abstain')),
    api_latency_ms bigint,
    created_at timestamptz not null default now(),
    completed_at timestamptz,
    unique (instance_id, chat_id, message_id, snapshot_hash)
);
create index moderation_decision_ready_idx on moderation_decision_jobs
    (instance_id, next_attempt_at, id) where status in ('pending','retry_wait','processing');
create table moderation_decision_budget (
    instance_id text not null,
    window_start timestamptz not null,
    calls integer not null check (calls >= 0),
    primary key (instance_id, window_start)
);
revoke all on moderation_text_observations, moderation_decision_jobs, moderation_decision_budget from public;
revoke all on sequence moderation_decision_jobs_id_seq from public;
