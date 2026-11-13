-- Снимок Decision отзывается при правке/удалении сообщения, а pending captcha
-- подхватывает existing setup worker для снятия только принадлежащего ей mute.
create function invalidate_decision_message_source() returns trigger language plpgsql as $$
begin
    if TG_OP='UPDATE' and old.text is not distinct from new.text
       and old.edited_at is not distinct from new.edited_at then return new; end if;
    update telegram_new_user_profile_audits a set risk_decision_job_id=null,
        risk_decision_message_id=null,risk_decision_score=0,risk_decision_signals='[]',
        risk_contact_score=0,risk_contact_signals='[]'
    where a.chat_id=old.chat_id and a.telegram_user_id=old.user_id
      and a.risk_decision_message_id=old.message_id;
    update telegram_risk_captcha_challenges c set
        status=case when status='failed' then 'pending' else status end,
        setup_next_attempt_at=now(),setup_error_kind='decision_recheck'
    where c.chat_id=old.chat_id and c.telegram_user_id=old.user_id
      and c.decision_job_id in (select id from moderation_decision_jobs
          where chat_id=old.chat_id and user_id=old.user_id and message_id=old.message_id)
      and c.status in ('pending','failed','preparing');
    if TG_OP='DELETE' then return old; end if;
    return new;
end $$;
create trigger invalidate_decision_message_source after update of text,edited_at or delete
    on telegram_messages for each row execute function invalidate_decision_message_source();

create or replace function revoke_decision_metrics_on_normal() returns trigger language plpgsql as $$
begin
    if new.label='not_spam' then
        update telegram_new_user_profile_audits set risk_decision_job_id=null,
            risk_decision_message_id=null,risk_decision_score=0,risk_decision_signals='[]',
            risk_contact_score=0,risk_contact_signals='[]'
        where chat_id=new.chat_id and telegram_user_id=new.telegram_user_id;
        update telegram_risk_captcha_challenges set setup_next_attempt_at=now(),
            status=case when status='failed' then 'pending' else status end,
            setup_error_kind='decision_recheck'
        where chat_id=new.chat_id and telegram_user_id=new.telegram_user_id
          and decision_job_id is not null and status in ('pending','failed','preparing');
    end if;
    return new;
end $$;
revoke all on function invalidate_decision_message_source() from public;
