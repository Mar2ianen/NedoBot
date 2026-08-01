alter table ask_runs
    add column if not exists render_captured_now timestamptz,
    add column if not exists render_dialect text,
    add column if not exists render_version text;
