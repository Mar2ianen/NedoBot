alter table telegram_new_user_profile_audits
    add column if not exists risk_profile text,
    add column if not exists risk_profile_version text,
    add column if not exists telegram_id_model_version text;

comment on column telegram_new_user_profile_audits.risk_profile is
    'Named moderation policy used for the baseline risk score';
comment on column telegram_new_user_profile_audits.risk_profile_version is
    'Stable policy version used to materialize the audit';
comment on column telegram_new_user_profile_audits.telegram_id_model_version is
    'Version of the Telegram ID signal model, when enabled by the policy';
