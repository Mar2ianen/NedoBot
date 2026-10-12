alter table telegram_risk_captcha_challenges
    add column trigger_source text not null default 'audit_score'
        check (trigger_source in ('audit_score', 'paid_dm_plus_offer_v1'));
