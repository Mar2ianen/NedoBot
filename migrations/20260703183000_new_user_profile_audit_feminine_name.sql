alter table telegram_new_user_profile_audits
    add column if not exists first_name_feminine_pattern boolean not null default false;
