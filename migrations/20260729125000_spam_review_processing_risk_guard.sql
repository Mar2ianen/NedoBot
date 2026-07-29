-- risk_score may be recalculated after a historical send or claim. Enforce the delivery
-- threshold only when a request is newly claimed for Telegram delivery.
alter table spam_review_requests
    drop constraint if exists spam_review_requests_low_risk_delivery_forbidden;

create or replace function spam_review_requests_reject_low_risk_processing()
returns trigger
language plpgsql
as $$
begin
    if new.notification_status = 'processing'
       and old.notification_status is distinct from 'processing'
       and new.risk_score < 70 then
        raise exception
            'cannot transition spam review request into processing with risk_score below 70'
            using errcode = 'check_violation';
    end if;

    return new;
end;
$$;

drop trigger if exists spam_review_requests_low_risk_processing_guard
    on spam_review_requests;

create trigger spam_review_requests_low_risk_processing_guard
before update of notification_status on spam_review_requests
for each row
execute function spam_review_requests_reject_low_risk_processing();
