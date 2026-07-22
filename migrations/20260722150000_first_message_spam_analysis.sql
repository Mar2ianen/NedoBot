alter table telegram_new_user_profile_audits
    add column if not exists first_message_marker_assessment jsonb,
    add column if not exists first_message_embedding vector(312),
    add column if not exists first_message_embedding_model text,
    add column if not exists first_message_spam_similarity double precision,
    add column if not exists first_message_template_matches integer not null default 0,
    add column if not exists first_message_analysis_at timestamptz;

create index if not exists telegram_new_user_profile_audits_first_message_embedding_idx
    on telegram_new_user_profile_audits using hnsw (first_message_embedding vector_cosine_ops)
    where first_message_embedding is not null;
