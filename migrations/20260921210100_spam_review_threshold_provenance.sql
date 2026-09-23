alter table spam_review_requests
    add column if not exists review_threshold integer not null default 70,
    add column if not exists notification_delivery_review_threshold integer;

comment on column spam_review_requests.review_threshold is
    'Risk profile review threshold captured with the score awaiting delivery.';

alter table spam_review_requests
    drop constraint if exists spam_review_requests_low_risk_delivery_forbidden;

create or replace function spam_review_requests_reject_low_risk_processing()
returns trigger
language plpgsql
as $$
begin
    if new.notification_status = 'processing'
       and old.notification_status is distinct from 'processing'
       and new.risk_score < new.review_threshold then
        raise exception
            'cannot transition spam review request into processing below its review threshold'
            using errcode = 'check_violation';
    end if;

    return new;
end;
$$;

drop index if exists spam_review_requests_notification_ready_idx;

create index spam_review_requests_notification_ready_idx
    on spam_review_requests (notification_next_attempt_at, id)
    where status = 'pending'
      and risk_score >= review_threshold
      and notification_status in ('pending', 'retry_wait');
