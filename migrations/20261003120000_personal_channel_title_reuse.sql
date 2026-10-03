alter table telegram_new_user_profile_audits
    add column if not exists personal_channel_title_reuse_count bigint not null default 0,
    add column if not exists personal_channel_title_reuse_spammer_count bigint not null default 0,
    add column if not exists personal_channel_title_reused_by_spammers boolean not null default false;
