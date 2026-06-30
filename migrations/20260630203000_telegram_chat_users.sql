create table if not exists telegram_chat_users (
    chat_id bigint not null,
    telegram_user_id bigint not null,
    first_seen_at timestamptz,
    last_seen_at timestamptz,
    first_message_id integer,
    last_message_id integer,
    message_count bigint not null default 0,
    reply_count bigint not null default 0,
    link_count bigint not null default 0,
    media_count bigint not null default 0,
    reply_to_channel_post_count bigint not null default 0,
    reply_to_bot_count bigint not null default 0,
    member_status text,
    is_admin boolean not null default false,
    is_present boolean,
    member_observed_at timestamptz,
    first_joined_at timestamptz,
    last_joined_at timestamptz,
    last_left_at timestamptz,
    last_invite_link text,
    via_chat_folder_invite_link boolean not null default false,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (chat_id, telegram_user_id)
);

create index if not exists telegram_chat_users_last_seen_idx
    on telegram_chat_users (chat_id, last_seen_at desc);

create index if not exists telegram_chat_users_message_count_idx
    on telegram_chat_users (chat_id, message_count desc);

insert into telegram_chat_users
    (
        chat_id, telegram_user_id, first_seen_at, last_seen_at,
        first_message_id, last_message_id, message_count, reply_count,
        link_count, media_count, reply_to_channel_post_count, reply_to_bot_count,
        member_status, is_admin, is_present, member_observed_at, updated_at
    )
select
    m.chat_id,
    m.user_id,
    min(coalesce(to_timestamp(nullif(m.raw_json #>> '{date}', '')::bigint), m.created_at)) as first_seen_at,
    max(coalesce(to_timestamp(nullif(m.raw_json #>> '{date}', '')::bigint), m.created_at)) as last_seen_at,
    (array_agg(m.message_id order by coalesce(to_timestamp(nullif(m.raw_json #>> '{date}', '')::bigint), m.created_at), m.message_id))[1] as first_message_id,
    (array_agg(m.message_id order by coalesce(to_timestamp(nullif(m.raw_json #>> '{date}', '')::bigint), m.created_at) desc, m.message_id desc))[1] as last_message_id,
    count(*)::bigint as message_count,
    count(*) filter (where m.reply_to_message_id is not null)::bigint as reply_count,
    count(*) filter (where m.has_links)::bigint as link_count,
    count(*) filter (where m.has_photo or m.has_video or m.has_document or m.has_audio or m.has_voice or m.has_sticker or m.has_animation)::bigint as media_count,
    count(*) filter (where m.reply_to_message_id in (select discussion_message_id from post_comment_jobs where discussion_chat_id = m.chat_id))::bigint as reply_to_channel_post_count,
    count(*) filter (where m.reply_to_message_id in (select bot_comment_message_id from post_comment_jobs where discussion_chat_id = m.chat_id))::bigint as reply_to_bot_count,
    s.status,
    coalesce(s.is_admin, false),
    s.is_present,
    s.observed_at,
    now()
from telegram_messages m
left join telegram_chat_member_snapshots s
    on s.chat_id = m.chat_id
   and s.telegram_user_id = m.user_id
where m.user_id is not null
  and m.source_channel_id is null
group by m.chat_id, m.user_id, s.status, s.is_admin, s.is_present, s.observed_at
on conflict (chat_id, telegram_user_id) do update set
    first_seen_at = least(telegram_chat_users.first_seen_at, excluded.first_seen_at),
    last_seen_at = greatest(telegram_chat_users.last_seen_at, excluded.last_seen_at),
    first_message_id = excluded.first_message_id,
    last_message_id = excluded.last_message_id,
    message_count = excluded.message_count,
    reply_count = excluded.reply_count,
    link_count = excluded.link_count,
    media_count = excluded.media_count,
    reply_to_channel_post_count = excluded.reply_to_channel_post_count,
    reply_to_bot_count = excluded.reply_to_bot_count,
    member_status = coalesce(excluded.member_status, telegram_chat_users.member_status),
    is_admin = excluded.is_admin,
    is_present = coalesce(excluded.is_present, telegram_chat_users.is_present),
    member_observed_at = coalesce(excluded.member_observed_at, telegram_chat_users.member_observed_at),
    updated_at = now();
