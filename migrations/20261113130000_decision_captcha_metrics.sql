-- Дополнительные метрики и durable handoff от Decision к существующей капче.
-- Старые rows/профили сохраняют прежнюю семантику при нулевом Decision-вкладе.
alter table telegram_new_user_profile_audits
    add column risk_decision_job_id bigint references moderation_decision_jobs(id) on delete set null,
    add column risk_decision_message_id integer,
    add column risk_decision_score integer not null default 0 check (risk_decision_score between 0 and 100),
    add column risk_decision_signals jsonb not null default '[]'::jsonb check (jsonb_typeof(risk_decision_signals)='array'),
    add column risk_contact_score integer not null default 0 check (risk_contact_score between 0 and 15),
    add column risk_contact_signals jsonb not null default '[]'::jsonb check (jsonb_typeof(risk_contact_signals)='array');

alter table moderation_decision_jobs
    add column action_status text not null default 'not_requested'
        check (action_status in ('not_requested','pending','processing','retry_wait','done','skipped','failed')),
    add column action_attempts integer not null default 0 check (action_attempts>=0),
    add column action_fence bigint not null default 0,
    add column action_next_attempt_at timestamptz not null default now(),
    add column action_lease_expires_at timestamptz,
    add column action_error_kind text,
    add column metrics_json jsonb;
create index moderation_decision_action_ready_idx on moderation_decision_jobs
    (instance_id, action_next_attempt_at, id)
    where action_status in ('pending','retry_wait','processing');

alter table telegram_risk_captcha_challenges
    alter column audit_job_id drop not null,
    add column decision_job_id bigint references moderation_decision_jobs(id),
    add column ban_on_failure boolean not null default false,
    add constraint risk_captcha_single_source check (num_nonnulls(audit_job_id,decision_job_id)=1);

-- Два классификатора одного текста не суммируются: берём максимум content-вклада.
-- Trigger сохраняет Decision при последующих legacy materialization/baseline updates.
create function merge_decision_risk_metrics() returns trigger language plpgsql as $$
begin
    if new.risk_decision_score>0 or new.risk_contact_score>0
       or (TG_OP='UPDATE' and (old.risk_decision_score>0 or old.risk_contact_score>0)) then
        new.risk_score := least(100, greatest(0,new.risk_baseline_score)
            + greatest(new.risk_first_message_score,new.risk_decision_score)
            + new.risk_avatar_score + new.risk_personal_channel_score
            + new.risk_external_score + new.risk_contact_score);
        new.risk_level := case when new.risk_score>=70 then 'high'
            when new.risk_score>=40 then 'medium' else 'low' end;
        new.risk_signal_breakdown := new.risk_baseline_signals || new.risk_first_message_signals
            || new.risk_avatar_signals || new.risk_personal_channel_signals || new.risk_external_signals
            || case when new.risk_decision_score>0 then jsonb_set(new.risk_decision_signals,
                '{0,coefficient}',to_jsonb(greatest(0,new.risk_decision_score-new.risk_first_message_score)),true)
                else '[]'::jsonb end || new.risk_contact_signals;
    end if;
    return new;
end $$;
create trigger merge_decision_risk_metrics before insert or update on telegram_new_user_profile_audits
    for each row execute function merge_decision_risk_metrics();

-- Ручная normal-разметка немедленно отзывает новые метрики. Queue/Telegram
-- отдельно перепроверяют её перед отправкой и перед истечением challenge.
create function revoke_decision_metrics_on_normal() returns trigger language plpgsql as $$
begin
    if new.label='not_spam' then
        update telegram_new_user_profile_audits set risk_decision_job_id=null,
            risk_decision_message_id=null,risk_decision_score=0,risk_decision_signals='[]',
            risk_contact_score=0,risk_contact_signals='[]'
        where chat_id=new.chat_id and telegram_user_id=new.telegram_user_id;
        update telegram_risk_captcha_challenges set setup_next_attempt_at=now()
        where chat_id=new.chat_id and telegram_user_id=new.telegram_user_id
          and decision_job_id is not null and status='pending';
    end if;
    return new;
end $$;
create trigger revoke_decision_metrics_on_normal after insert on spam_label_events
    for each row execute function revoke_decision_metrics_on_normal();

-- Новых публичных views/GRANT нет; raw Decision и action outbox остаются private.
revoke all on function merge_decision_risk_metrics(), revoke_decision_metrics_on_normal() from public;
