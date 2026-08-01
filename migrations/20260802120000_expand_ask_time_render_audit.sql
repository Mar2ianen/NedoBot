alter table ask_runs
    add column if not exists render_timezone text,
    add column if not exists renderer_revision text,
    add column if not exists rendered_markdown text,
    add column if not exists delivery_certainty text,
    add column if not exists delivery_outcome text;
