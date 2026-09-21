alter table new_user_audit_jobs
    add column if not exists review_threshold integer not null default 70;

comment on column new_user_audit_jobs.review_threshold is
    'Risk profile review threshold captured when this audit job was created.';
