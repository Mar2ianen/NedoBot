-- The worker must compare this immutable claim snapshot with the current request
-- immediately before the Telegram side effect. Existing processing claims are
-- intentionally left without a snapshot; they are reclaimed before delivery.
alter table spam_review_requests
    add column if not exists notification_delivery_risk_score integer,
    add column if not exists notification_delivery_risk_signals jsonb;
