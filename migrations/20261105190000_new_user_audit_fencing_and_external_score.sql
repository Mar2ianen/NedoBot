alter table new_user_audit_jobs
    add column if not exists generation_fence bigint not null default 0
        check (generation_fence >= 0),
    add column if not exists materialization_fence bigint not null default 0
        check (materialization_fence >= 0),
    add column if not exists audit_generation bigint not null default 0
        check (audit_generation >= 0);

alter table telegram_new_user_profile_audits
    add column if not exists risk_external_score integer not null default 0
        check (risk_external_score >= 0),
    add column if not exists risk_external_signals jsonb not null default '[]'::jsonb;
