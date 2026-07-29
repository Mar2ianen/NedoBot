alter table spam_review_requests
    drop constraint if exists spam_review_requests_notification_status_check;

alter table spam_review_requests
    add constraint spam_review_requests_notification_status_check
    check (notification_status in ('pending', 'processing', 'retry_wait', 'sent', 'failed'));

-- До lifecycle доставки notified_at выставлялся при создании карточки, а не после
-- подтверждённой отправки. Без достоверного delivery state безопаснее считать такие
-- historical pending записи уже показанными, чем повторно слать их владельцу. Граница
-- по журналу миграций не затрагивает новые заявки, созданные после rollout lifecycle.
with lifecycle as (
    select installed_on
    from _sqlx_migrations
    where version = 20260729100000
)
update spam_review_requests request
set notification_status = 'sent',
    notified_risk_score = request.risk_score,
    notified_risk_signals = request.risk_signals,
    notification_processing_started_at = null,
    notification_lease_expires_at = null,
    notification_error_kind = null
where request.status = 'pending'
  and request.notified_at < (select installed_on from lifecycle)
  and request.notification_attempts = 0
  and request.notified_risk_score is null
  and request.notified_risk_signals is null;
