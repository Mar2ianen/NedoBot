alter table telegram_new_user_profile_audits
    add column if not exists risk_baseline_score integer not null default 0
        check (risk_baseline_score >= 0),
    add column if not exists risk_baseline_signals jsonb not null default '[]'::jsonb,
    add column if not exists risk_first_message_score integer not null default 0
        check (risk_first_message_score >= 0),
    add column if not exists risk_first_message_signals jsonb not null default '[]'::jsonb,
    add column if not exists risk_avatar_score integer not null default 0
        check (risk_avatar_score >= 0),
    add column if not exists risk_avatar_signals jsonb not null default '[]'::jsonb;

-- Existing rows were materialized as one legacy score. Preserve that score as
-- the baseline until a unified audit replaces all components atomically.
update telegram_new_user_profile_audits
set risk_baseline_score = risk_score,
    risk_baseline_signals = risk_signal_breakdown
where risk_baseline_score = 0
  and risk_first_message_score = 0
  and risk_avatar_score = 0
  and risk_score > 0;
