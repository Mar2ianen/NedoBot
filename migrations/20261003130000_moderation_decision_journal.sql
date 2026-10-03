-- Единый журнал модерационных решений для настройки порогов и /modlog.
-- Только чтение поверх существующих таблиц: решения из review-кнопок,
-- /notspam, /report, ручных команд и будущих auto-enforcement пишутся
-- через features::labels и manual_moderation, сюда попадают автоматически.
create or replace view moderation_decision_journal as
select
    e.created_at as decided_at,
    e.chat_id,
    e.telegram_user_id as target_user_id,
    e.operator_telegram_user_id as actor_user_id,
    ('label:' || e.label) as kind,
    coalesce(nullif(e.subtype, ''), '-') || ' | ' || left(e.reason, 200) as detail,
    ('spam_label:' || e.source) as source
from public.spam_label_events e
union all
select
    v.created_at as decided_at,
    v.chat_id,
    v.target_user_id,
    v.actor_user_id,
    ('manual:' || v.event) as kind,
    coalesce(a.action, '-') || ' | ' || left(coalesce(v.reason, ''), 200) as detail,
    'manual_moderation' as source
from public.manual_moderation_events v
left join public.manual_moderation_actions a on a.id = v.action_id
union all
select
    r.resolved_at as decided_at,
    r.chat_id,
    r.reported_user_id as target_user_id,
    r.resolved_by_user_id as actor_user_id,
    ('report:' || r.resolution) as kind,
    left(coalesce(nullif(r.reason, ''), '-'), 200) as detail,
    'telegram_reports' as source
from public.telegram_reports r
where r.resolution in ('accepted', 'rejected');
