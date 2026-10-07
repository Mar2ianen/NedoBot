alter table public.post_history_entries
    add column if not exists embedding_gemma2 vector(512),
    add column if not exists embedding_gemma2_model text;

do $$
declare
    legacy_embedding_check text;
begin
    select conname
      into legacy_embedding_check
      from pg_constraint
     where conrelid = 'public.post_history_entries'::regclass
       and contype = 'c'
       and lower(pg_get_constraintdef(oid)) like '%embedding is not null%'
     limit 1;

    if legacy_embedding_check is not null then
        execute 'alter table public.post_history_entries drop constraint '
            || quote_ident(legacy_embedding_check);
    end if;
end $$;

alter table public.post_history_entries
    add constraint post_history_entries_ready_embedding_check
    check ((status = 'ready') = (
        summary is not null and (embedding is not null or embedding_gemma2 is not null)
    ));

create index if not exists post_history_entries_gemma2_ready_hnsw_idx
    on public.post_history_entries using hnsw (embedding_gemma2 vector_cosine_ops)
    where status = 'ready' and embedding_gemma2 is not null;

alter table public.telegram_new_user_profile_audits
    add column if not exists first_message_embedding_gemma2 vector(512),
    add column if not exists first_message_embedding_gemma2_model text;

create index if not exists telegram_new_user_profile_audits_first_message_gemma2_hnsw_idx
    on public.telegram_new_user_profile_audits using hnsw (first_message_embedding_gemma2 vector_cosine_ops)
    where first_message_embedding_gemma2 is not null;


-- Keep the previous 768d chat table untouched for rollback while Gemma 2 is
-- backfilled into a parallel 512d queue and made the only active read path.
create table public.telegram_message_embeddings_gemma2 (
    chat_id bigint not null,
    message_id integer not null,
    embedding vector(512),
    embedding_model text,
    status text not null default 'pending'
        check (status in ('pending', 'processing', 'ready', 'retry_wait', 'failed', 'ignored')),
    attempts integer not null default 0,
    next_attempt_at timestamptz not null default now(),
    processing_started_at timestamptz,
    lease_expires_at timestamptz,
    error_kind text,
    lease_reclaim_count integer not null default 0 check (lease_reclaim_count >= 0),
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (chat_id, message_id),
    foreign key (chat_id, message_id)
        references public.telegram_messages(chat_id, message_id) on delete cascade,
    check ((status = 'ready') = (embedding is not null and embedding_model is not null))
);

create index telegram_message_embeddings_gemma2_claim_idx
    on public.telegram_message_embeddings_gemma2 (next_attempt_at, created_at)
    where status in ('pending', 'retry_wait');

create index telegram_message_embeddings_gemma2_ready_hnsw_idx
    on public.telegram_message_embeddings_gemma2 using hnsw (embedding vector_cosine_ops)
    where status = 'ready';

-- This table contains only avatars queued after an explicit spammer label.
-- It stores the embedding and Telegram's opaque file identifiers, never image
-- bytes. Removing a spam label deletes the user's dataset rows.
create table public.spammer_avatar_embeddings (
    chat_id bigint not null,
    telegram_user_id bigint not null,
    avatar_file_unique_id text not null,
    avatar_file_id text,
    embedding vector(512),
    embedding_model text,
    status text not null default 'pending'
        check (status in ('pending', 'processing', 'retry_wait', 'ready', 'failed', 'ignored')),
    attempts integer not null default 0,
    next_attempt_at timestamptz not null default now(),
    processing_started_at timestamptz,
    lease_expires_at timestamptz,
    error_kind text,
    lease_reclaim_count integer not null default 0 check (lease_reclaim_count >= 0),
    confirmed_at timestamptz not null default now(),
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (chat_id, telegram_user_id, avatar_file_unique_id),
    foreign key (chat_id, telegram_user_id)
        references public.telegram_chat_users(chat_id, telegram_user_id) on delete cascade,
    check ((status = 'ready') = (embedding is not null and embedding_model is not null)),
    check (status = 'ready' or avatar_file_id is not null),
    check (status <> 'ready' or avatar_file_id is null)
);

create index spammer_avatar_embeddings_claim_idx
    on public.spammer_avatar_embeddings (next_attempt_at, created_at)
    where status in ('pending', 'retry_wait');

create index spammer_avatar_embeddings_ready_hnsw_idx
    on public.spammer_avatar_embeddings using hnsw (embedding vector_cosine_ops)
    where status = 'ready';
