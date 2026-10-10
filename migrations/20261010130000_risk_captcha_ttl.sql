alter table telegram_risk_captcha_challenges
    add column expires_at timestamptz;

alter table telegram_risk_captcha_challenges
    drop constraint telegram_risk_captcha_challenges_status_check;

alter table telegram_risk_captcha_challenges
    add constraint telegram_risk_captcha_challenges_status_check
    check (status in (
        'preparing', 'setting_up', 'pending', 'solving', 'expiring',
        'passed', 'failed', 'overridden', 'setup_failed', 'expired'
    ));

create index telegram_risk_captcha_expiry_ready_idx
    on telegram_risk_captcha_challenges (expires_at, created_at)
    where status in ('pending', 'failed');

create index telegram_risk_captcha_expiry_lease_idx
    on telegram_risk_captcha_challenges (setup_lease_expires_at, created_at)
    where status = 'expiring';
