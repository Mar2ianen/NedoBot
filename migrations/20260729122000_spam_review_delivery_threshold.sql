-- Audit/snapshot для нового пользователя допускается при любом score, но Telegram delivery
-- нельзя даже claim-ить ниже порога. Это DB safety net для всех caller-ов send_review.
do $$
begin
    if not exists (
        select 1
        from pg_constraint
        where conrelid = 'spam_review_requests'::regclass
          and conname = 'spam_review_requests_low_risk_delivery_forbidden'
    ) then
        alter table spam_review_requests
            add constraint spam_review_requests_low_risk_delivery_forbidden
            check (
                risk_score >= 70
                or (notification_attempts = 0 and notification_message_id is null)
            );
    end if;
end $$;
