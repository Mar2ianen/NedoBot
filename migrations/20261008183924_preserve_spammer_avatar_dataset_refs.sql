alter table public.spammer_avatar_embeddings
    add column if not exists dataset_avatar_file_id text;

comment on column public.spammer_avatar_embeddings.dataset_avatar_file_id is
    'Telegram avatar file reference retained for an explicitly labeled spammer dataset; rows are removed when the spam label is removed.';
