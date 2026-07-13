alter table llm_generations
    add column if not exists used_search_result_id integer;
