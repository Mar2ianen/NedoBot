create table public.telegram_message_embeddings (
    chat_id bigint not null,
    message_id integer not null,
    embedding vector(312),
    embedding_model text,
    status text not null default 'pending'
        check (status in ('pending', 'processing', 'ready', 'retry_wait', 'failed', 'ignored')),
    attempts integer not null default 0,
    next_attempt_at timestamptz not null default now(),
    processing_started_at timestamptz,
    lease_expires_at timestamptz,
    error_kind text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (chat_id, message_id),
    foreign key (chat_id, message_id)
        references public.telegram_messages(chat_id, message_id) on delete cascade,
    check ((status = 'ready') = (embedding is not null and embedding_model is not null))
);

create index telegram_message_embeddings_claim_idx
    on public.telegram_message_embeddings (next_attempt_at, created_at)
    where status in ('pending', 'retry_wait');

create index telegram_message_embeddings_ready_hnsw_idx
    on public.telegram_message_embeddings using hnsw (embedding vector_cosine_ops)
    where status = 'ready';

create table public.chat_research_runs (
    id bigserial primary key,
    post_comment_job_id bigint unique references public.post_comment_jobs(id) on delete cascade,
    plan jsonb,
    retrieval_candidates jsonb not null default '[]'::jsonb,
    expanded_contexts jsonb not null default '[]'::jsonb,
    used_chat_message_ids integer[] not null default '{}',
    evidence_rejection_reason text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
