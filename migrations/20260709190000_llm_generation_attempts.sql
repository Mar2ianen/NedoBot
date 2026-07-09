alter table llm_generations
    add column if not exists attempts jsonb not null default '[]'::jsonb;
