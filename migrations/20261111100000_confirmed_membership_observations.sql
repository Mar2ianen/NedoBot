-- Это время подтверждённого наблюдения членства, а не восстановленная дата входа.
-- Отсутствие подтверждения никогда не означает «никогда не вступал».
alter table public.telegram_chat_users
    add column membership_first_confirmed_at timestamptz,
    add column membership_last_confirmed_at timestamptz,
    add constraint telegram_chat_users_membership_observation_pair check (
        (membership_first_confirmed_at is null and membership_last_confirmed_at is null)
        or (membership_first_confirmed_at is not null
            and membership_last_confirmed_at is not null
            and membership_first_confirmed_at <= membership_last_confirmed_at)
    );

with service_messages as materialized (
    select chat_id, raw_json, created_at
    from public.telegram_messages
    where chat_id < 0 and source_channel_id is null and not is_forwarded
      and (raw_json ? 'new_chat_members' or raw_json ? 'left_chat_member')
), observations as (
    select chat_id, telegram_user_id, event_at as observed_at
    from public.telegram_chat_member_events
    where chat_id < 0 and (
        old_status in ('creator','owner','administrator','member')
        or new_status in ('creator','owner','administrator','member')
        or (old_status = 'restricted'
            and raw_json #> '{old_chat_member,is_member}' = 'true'::jsonb)
        or (new_status = 'restricted'
            and raw_json #> '{new_chat_member,is_member}' = 'true'::jsonb)
    )
    union all
    select chat_id, telegram_user_id, observed_at
    from public.telegram_chat_member_snapshots
    where chat_id < 0 and is_present and (
        status in ('creator','owner','administrator','member')
        or (status = 'restricted' and (
            raw_json -> 'is_member' = 'true'::jsonb
            or raw_json #> '{new_chat_member,is_member}' = 'true'::jsonb
        ))
    )
    union all
    select m.chat_id,
           case when u ->> 'id' ~ '^[1-9][0-9]{0,17}$' then (u ->> 'id')::bigint end,
           m.created_at
    from service_messages m
    cross join lateral jsonb_array_elements(
        case when jsonb_typeof(m.raw_json -> 'new_chat_members') = 'array'
             then m.raw_json -> 'new_chat_members' else '[]'::jsonb end
    ) u
    where u ->> 'id' ~ '^[1-9][0-9]{0,17}$'
    union all
    select chat_id,
           case when raw_json #>> '{left_chat_member,id}' ~ '^[1-9][0-9]{0,17}$'
                then (raw_json #>> '{left_chat_member,id}')::bigint end,
           created_at
    from service_messages
    where raw_json #>> '{left_chat_member,id}' ~ '^[1-9][0-9]{0,17}$'
), confirmed as (
    select chat_id, telegram_user_id, min(observed_at) as first_at,
           max(observed_at) as last_at
    from observations
    where telegram_user_id > 0
    group by chat_id, telegram_user_id
)
insert into public.telegram_chat_users
    (chat_id, telegram_user_id, membership_first_confirmed_at, membership_last_confirmed_at)
select chat_id, telegram_user_id, first_at, last_at from confirmed
on conflict (chat_id, telegram_user_id) do update set
    membership_first_confirmed_at = least(telegram_chat_users.membership_first_confirmed_at,
                                         excluded.membership_first_confirmed_at),
    membership_last_confirmed_at = greatest(telegram_chat_users.membership_last_confirmed_at,
                                           excluded.membership_last_confirmed_at);

comment on column public.telegram_chat_users.membership_first_confirmed_at is
    'First retained positive membership observation; not a proven join date. NULL means unknown history, never proof of never joining.';
comment on column public.telegram_chat_users.membership_last_confirmed_at is
    'Last retained positive membership observation, preserved across left/banned snapshots.';
