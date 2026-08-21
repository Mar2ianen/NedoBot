create table if not exists public.spam_label_events (
    id bigserial primary key,
    chat_id bigint not null,
    telegram_user_id bigint not null,
    label text not null
        check (label in ('spam', 'not_spam')),
    subtype text,
    source text not null
        check (source in ('owner_review', 'owner_manual', 'legacy_backfill', 'system')),
    reason text not null,
    evidence jsonb not null default '[]'::jsonb,
    operator_telegram_user_id bigint,
    created_at timestamptz not null default now()
);

create index if not exists spam_label_events_subject_idx
    on public.spam_label_events (chat_id, telegram_user_id, created_at desc, id desc);

create index if not exists spam_label_events_label_idx
    on public.spam_label_events (chat_id, label, subtype, created_at desc);

-- Preserve the current manually maintained truth as the first durable event for
-- every known spammer. The mutable columns remain a compatibility projection.
insert into public.spam_label_events (
    chat_id,
    telegram_user_id,
    label,
    subtype,
    source,
    reason,
    evidence,
    created_at
)
select
    u.chat_id,
    u.telegram_user_id,
    'spam',
    coalesce(nullif(trim(u.spam_type), ''), 'legacy_untyped'),
    'legacy_backfill',
    coalesce(nullif(trim(u.spam_reason), ''), 'Legacy spammer flag'),
    jsonb_build_object(
        'legacy_spam_type', u.spam_type,
        'spam_score', u.spam_score,
        'spam_last_marked_at', u.spam_last_marked_at,
        'legacy_projection', 'telegram_chat_users.is_spammer'
    ),
    coalesce(u.spam_last_marked_at, now())
from public.telegram_chat_users u
where u.is_spammer
  and not exists (
      select 1
      from public.spam_label_events e
      where e.chat_id = u.chat_id
        and e.telegram_user_id = u.telegram_user_id
        and e.source = 'legacy_backfill'
        and e.label = 'spam'
  );

-- Keep any already completed false-positive reviews as negative examples too.
insert into public.spam_label_events (
    chat_id,
    telegram_user_id,
    label,
    subtype,
    source,
    reason,
    evidence,
    operator_telegram_user_id,
    created_at
)
select
    r.chat_id,
    r.telegram_user_id,
    'not_spam',
    'confirmed_normal',
    'owner_review',
    'Owner rejected spam review',
    coalesce(r.risk_signals, '[]'::jsonb),
    r.reviewed_by_user_id,
    coalesce(r.reviewed_at, now())
from public.spam_review_requests r
where r.status = 'confirmed_not_spam'
  and not exists (
      select 1
      from public.spam_label_events e
      where e.chat_id = r.chat_id
        and e.telegram_user_id = r.telegram_user_id
        and e.source = 'owner_review'
        and e.label = 'not_spam'
        and e.subtype = 'confirmed_normal'
  );
