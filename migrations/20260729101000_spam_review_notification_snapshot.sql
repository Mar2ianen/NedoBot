alter table spam_review_requests
    add column if not exists notified_risk_score integer,
    add column if not exists notified_risk_signals jsonb;
