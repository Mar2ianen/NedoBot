use sqlx::PgPool;

use crate::features::stats::types::{ChatStatsSummary, StatsPeriod};

/// Telegram's built-in service account. It is excluded from human activity rankings.
pub const TELEGRAM_SERVICE_USER_ID: i64 = 777_000;

#[derive(Debug, Clone, sqlx::FromRow)]
struct ChatStatsSummaryRow {
    start_label: String,
    messages: i64,
    active_users: i64,
    replies: i64,
    links: i64,
    media: i64,
    channel_posts: i64,
    bot_comments: i64,
    replies_to_bot: i64,
    reaction_events: i64,
    reaction_count_updates: i64,
    bot_comment_reactions: i64,
    joins: i64,
    leaves: i64,
}

impl From<ChatStatsSummaryRow> for ChatStatsSummary {
    fn from(row: ChatStatsSummaryRow) -> Self {
        Self {
            start_label: row.start_label,
            messages: row.messages,
            active_users: row.active_users,
            replies: row.replies,
            links: row.links,
            media: row.media,
            channel_posts: row.channel_posts,
            bot_comments: row.bot_comments,
            replies_to_bot: row.replies_to_bot,
            reaction_events: row.reaction_events,
            reaction_count_updates: row.reaction_count_updates,
            bot_comment_reactions: row.bot_comment_reactions,
            joins: row.joins,
            leaves: row.leaves,
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AttractionMetrics {
    pub messages_5m: String,
    pub messages_30m: String,
    pub users_30m: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PeriodTopUser {
    pub user_id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub messages: i64,
    pub replies: i64,
    pub links: i64,
    pub media: i64,
    pub status: String,
    pub is_admin: bool,
    pub is_present: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BotCommentStats {
    pub source_message_id: i32,
    pub response: String,
    pub messages_30m: i64,
    pub direct_replies: i64,
    pub reactions: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TopMessage {
    pub user_id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub is_bot: bool,
    pub status: String,
    pub is_admin: bool,
    pub is_present: bool,
    pub messages: i64,
    pub replies: i64,
    pub media: i64,
    pub voices: i64,
    pub links: i64,
    pub reactions_received: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TopReactedMessage {
    pub message_id: i32,
    pub user_id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub is_bot: bool,
    pub status: String,
    pub is_admin: bool,
    pub is_present: bool,
    pub text: Option<String>,
    pub has_photo: bool,
    pub has_video: bool,
    pub has_document: bool,
    pub has_audio: bool,
    pub has_voice: bool,
    pub has_sticker: bool,
    pub has_animation: bool,
    pub total_count: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserProfile {
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub is_bot: bool,
    pub bio: Option<String>,
    pub profile_photo_file_id: Option<String>,
    pub profile_photo_file_unique_id: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChatMemberSnapshot {
    pub status: String,
    pub is_admin: bool,
    pub is_present: bool,
    pub observed_at: Option<String>,
    pub written_tag: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChatUserStats {
    pub first_seen_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub first_message_id: Option<i32>,
    pub last_message_id: Option<i32>,
    pub first_seen_days_ago: Option<i64>,
    pub last_seen_days_ago: Option<i64>,
    pub messages: i64,
    pub replies: i64,
    pub links: i64,
    pub media: i64,
    pub replies_to_channel_posts: i64,
    pub replies_to_bot: i64,
    pub voices: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserTotals {
    pub messages: i64,
    pub replies: i64,
    pub links: i64,
    pub media: i64,
    pub post_comments: i64,
    pub replies_to_bot: i64,
    pub active_days: i64,
    pub voices: i64,
}

pub async fn chat_stats_summary(
    pool: &PgPool,
    discussion_chat_id: i64,
    period: StatsPeriod,
) -> anyhow::Result<ChatStatsSummary> {
    let sql = format!(
        r#"
        with bounds as (
            select {} as start_at, now() as end_at
        ),
        messages as (
            select m.* from telegram_messages m, bounds b
            where m.chat_id = $1 and m.created_at >= b.start_at and m.created_at < b.end_at
        ),
        bot_comments as (
            select j.* from post_comment_jobs j, bounds b
            where j.discussion_chat_id = $1 and j.created_at >= b.start_at and j.created_at < b.end_at
        ),
        reactions as (
            select r.* from telegram_message_reactions r, bounds b
            where r.chat_id = $1 and r.event_at >= b.start_at and r.event_at < b.end_at
        ),
        reaction_counts as (
            select rc.* from telegram_message_reaction_counts rc, bounds b
            where rc.chat_id = $1 and rc.event_at >= b.start_at and rc.event_at < b.end_at
        ),
        member_events as (
            select e.* from telegram_chat_member_events e, bounds b
            where e.chat_id = $1 and e.event_at >= b.start_at and e.event_at < b.end_at
        )
        select
            to_char((select start_at from bounds) at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI') as start_label,
            count(*)::bigint as messages,
            count(distinct user_id) filter (where source_channel_id is null and coalesce(user_id, 0) <> $2)::bigint as active_users,
            count(*) filter (where reply_to_message_id is not null)::bigint as replies,
            count(*) filter (where has_links)::bigint as links,
            count(*) filter (where has_photo or has_video or has_document or has_audio or has_voice or has_sticker or has_animation)::bigint as media,
            count(*) filter (where is_automatic_forward)::bigint as channel_posts,
            (select count(*) from bot_comments)::bigint as bot_comments,
            (select count(*) from messages m join bot_comments j on m.reply_to_message_id = j.bot_comment_message_id)::bigint as replies_to_bot,
            (select count(*) from reactions)::bigint as reaction_events,
            (select count(*) from reaction_counts)::bigint as reaction_count_updates,
            (select coalesce(sum(rc.total_count), 0)::bigint from reaction_counts rc join post_comment_jobs j on j.discussion_chat_id = rc.chat_id and j.bot_comment_message_id = rc.message_id)::bigint as bot_comment_reactions,
            (select count(*) from member_events where old_status in ('left', 'banned') and new_status not in ('left', 'banned'))::bigint as joins,
            (select count(*) from member_events where old_status not in ('left', 'banned') and new_status in ('left', 'banned'))::bigint as leaves
        from messages
        "#,
        period_start_sql(period)
    );

    let row: ChatStatsSummaryRow = sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .bind(TELEGRAM_SERVICE_USER_ID)
        .fetch_one(pool)
        .await?;
    Ok(row.into())
}

pub async fn chat_attraction_metrics(
    pool: &PgPool,
    discussion_chat_id: i64,
    period: StatsPeriod,
) -> anyhow::Result<AttractionMetrics> {
    let sql = format!(
        r#"
        with bounds as (select {} as start_at, now() as end_at),
        metrics as (
            select j.source_message_id,
                   count(m.*) filter (where m.created_at <= j.created_at + interval '5 minutes' and coalesce(m.text,'') !~ '^/') as messages_5m,
                   count(m.*) filter (where m.created_at <= j.created_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/') as messages_30m,
                   count(distinct m.user_id) filter (where m.created_at <= j.created_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/') as users_30m
            from post_comment_jobs j
            left join telegram_messages m on m.chat_id = j.discussion_chat_id
                and m.created_at > j.created_at and m.created_at <= j.created_at + interval '30 minutes'
                and m.message_id <> j.bot_comment_message_id and m.source_channel_id is null
            where j.discussion_chat_id = $1
              and j.created_at >= (select start_at from bounds) and j.created_at < (select end_at from bounds)
            group by j.source_message_id, j.created_at, j.bot_comment_message_id
        )
        select coalesce(round(avg(messages_5m)::numeric, 2), 0)::text as messages_5m,
               coalesce(round(avg(messages_30m)::numeric, 2), 0)::text as messages_30m,
               coalesce(round(avg(users_30m)::numeric, 2), 0)::text as users_30m
        from metrics
        "#,
        period_start_sql(period)
    );

    sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .fetch_one(pool)
        .await
        .map_err(Into::into)
}

pub async fn period_top_users(
    pool: &PgPool,
    discussion_chat_id: i64,
    period: StatsPeriod,
    limit: i64,
) -> anyhow::Result<Vec<PeriodTopUser>> {
    let sql = format!(
        r#"
        with bounds as (select {} as start_at, now() as end_at)
        select m.user_id, p.username, p.first_name, p.last_name,
               count(*)::bigint as messages,
               count(*) filter (where m.reply_to_message_id is not null)::bigint as replies,
               count(*) filter (where m.has_links)::bigint as links,
               count(*) filter (where m.has_photo or m.has_video or m.has_document or m.has_audio or m.has_voice or m.has_sticker or m.has_animation)::bigint as media,
               coalesce(s.status, 'unknown') as status, coalesce(s.is_admin, false) as is_admin,
               coalesce(s.is_present, false) as is_present
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = m.chat_id and s.telegram_user_id = m.user_id
        where m.chat_id = $1 and m.user_id is not null and m.source_channel_id is null
          and m.user_id <> $2 and coalesce(p.is_bot, false) = false
          and m.created_at >= (select start_at from bounds) and m.created_at < (select end_at from bounds)
        group by m.user_id, p.username, p.first_name, p.last_name, s.status, s.is_admin, s.is_present
        order by messages desc
        limit $3
        "#,
        period_start_sql(period)
    );

    sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .bind(TELEGRAM_SERVICE_USER_ID)
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(Into::into)
}

pub async fn bot_comments_for_period(
    pool: &PgPool,
    discussion_chat_id: i64,
    period: StatsPeriod,
    limit: i64,
) -> anyhow::Result<Vec<BotCommentStats>> {
    let sql = format!(
        r#"
        with bounds as (select {} as start_at, now() as end_at)
        select j.source_message_id, coalesce(g.response, '') as response,
               count(m.*) filter (where m.created_at <= j.created_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/')::bigint as messages_30m,
               count(m.*) filter (where m.reply_to_message_id = j.bot_comment_message_id)::bigint as direct_replies,
               coalesce(max(rc.total_count), 0)::bigint as reactions
        from post_comment_jobs j
        left join llm_generations g on g.post_comment_job_id = j.id
        left join telegram_messages m on m.chat_id = j.discussion_chat_id
            and m.created_at > j.created_at and m.created_at <= j.created_at + interval '30 minutes'
            and m.message_id <> j.bot_comment_message_id and m.source_channel_id is null
        left join telegram_message_reaction_counts rc on rc.chat_id = j.discussion_chat_id and rc.message_id = j.bot_comment_message_id
        where j.discussion_chat_id = $1
          and j.created_at >= (select start_at from bounds) and j.created_at < (select end_at from bounds)
        group by j.source_message_id, g.response
        order by messages_30m desc, direct_replies desc, reactions desc
        limit $2
        "#,
        period_start_sql(period)
    );

    sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(Into::into)
}

pub async fn top_message_users(
    pool: &PgPool,
    discussion_chat_id: i64,
    limit: i64,
) -> anyhow::Result<Vec<TopMessage>> {
    sqlx::query_as(
        r#"
        select m.user_id, p.username,
               coalesce(nullif(case when p.first_name = 'пользователь' then '' else p.first_name end, ''), raw_name.display_name, 'скрытый пользователь') as first_name,
               p.last_name, coalesce(p.is_bot, false) as is_bot, coalesce(s.status, 'unknown') as status,
               coalesce(s.is_admin, false) as is_admin, coalesce(s.is_present, false) as is_present,
               count(*)::bigint as messages, count(*) filter (where m.reply_to_message_id is not null)::bigint as replies,
               count(*) filter (where m.has_photo or m.has_video or m.has_document or m.has_audio or m.has_voice or m.has_sticker or m.has_animation)::bigint as media,
               count(*) filter (where m.has_voice)::bigint as voices, count(*) filter (where m.has_links)::bigint as links,
               coalesce(sum(rc.total_count), 0)::bigint as reactions_received
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = m.chat_id and s.telegram_user_id = m.user_id
        left join telegram_message_reaction_counts rc on rc.chat_id = m.chat_id and rc.message_id = m.message_id
        left join lateral (
            select coalesce(nullif(tm.raw_json #>> '{from,first_name}', ''), nullif(tm.raw_json ->> 'from', '')) as display_name
            from telegram_messages tm
            where tm.chat_id = m.chat_id and tm.user_id = m.user_id
              and coalesce(nullif(tm.raw_json #>> '{from,first_name}', ''), nullif(tm.raw_json ->> 'from', '')) is not null
            order by tm.created_at desc limit 1
        ) raw_name on true
        where m.chat_id = $1 and m.user_id is not null and m.source_channel_id is null
          and m.user_id <> $2 and coalesce(p.is_bot, false) = false
        group by m.user_id, p.username, p.first_name, p.last_name, p.is_bot, s.status, s.is_admin, s.is_present, raw_name.display_name
        order by messages desc, reactions_received desc
        limit $3
        "#,
    )
    .bind(discussion_chat_id)
    .bind(TELEGRAM_SERVICE_USER_ID)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn top_reacted_messages(
    pool: &PgPool,
    discussion_chat_id: i64,
    limit: i64,
) -> anyhow::Result<Vec<TopReactedMessage>> {
    sqlx::query_as(
        r#"
        select m.message_id, m.user_id, p.username,
               coalesce(nullif(case when p.first_name = 'пользователь' then '' else p.first_name end, ''), raw_name.display_name, 'скрытый пользователь') as first_name,
               p.last_name, coalesce(p.is_bot, false) as is_bot, coalesce(s.status, 'unknown') as status,
               coalesce(s.is_admin, false) as is_admin, coalesce(s.is_present, false) as is_present,
               m.text, m.has_photo, m.has_video, m.has_document, m.has_audio, m.has_voice, m.has_sticker, m.has_animation, rc.total_count
        from telegram_message_reaction_counts rc
        join telegram_messages m on m.chat_id = rc.chat_id and m.message_id = rc.message_id
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = m.chat_id and s.telegram_user_id = m.user_id
        left join lateral (
            select coalesce(nullif(tm.raw_json #>> '{from,first_name}', ''), nullif(tm.raw_json ->> 'from', '')) as display_name
            from telegram_messages tm
            where tm.chat_id = m.chat_id and tm.user_id = m.user_id
              and coalesce(nullif(tm.raw_json #>> '{from,first_name}', ''), nullif(tm.raw_json ->> 'from', '')) is not null
            order by tm.created_at desc limit 1
        ) raw_name on true
        where rc.chat_id = $1 and rc.total_count > 0 and m.user_id is not null and m.source_channel_id is null
          and m.user_id <> $2 and coalesce(p.is_bot, false) = false
        order by rc.total_count desc, m.created_at desc
        limit $3
        "#,
    )
    .bind(discussion_chat_id)
    .bind(TELEGRAM_SERVICE_USER_ID)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn top_message_user_ids(
    pool: &PgPool,
    discussion_chat_id: i64,
    limit: i64,
) -> anyhow::Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        r#"
        select m.user_id from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1 and m.user_id is not null and m.source_channel_id is null
          and m.user_id <> $2 and coalesce(p.is_bot, false) = false
        group by m.user_id order by count(*) desc limit $3
        "#,
    )
    .bind(discussion_chat_id)
    .bind(TELEGRAM_SERVICE_USER_ID)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(user_id,)| user_id).collect())
}

pub async fn top_reacted_user_ids(
    pool: &PgPool,
    discussion_chat_id: i64,
    limit: i64,
) -> anyhow::Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        r#"
        select m.user_id from telegram_message_reaction_counts rc
        join telegram_messages m on m.chat_id = rc.chat_id and m.message_id = rc.message_id
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where rc.chat_id = $1 and rc.total_count > 0 and m.user_id is not null and m.source_channel_id is null
          and m.user_id <> $2 and coalesce(p.is_bot, false) = false
        group by m.user_id order by max(rc.total_count) desc limit $3
        "#,
    )
    .bind(discussion_chat_id)
    .bind(TELEGRAM_SERVICE_USER_ID)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(user_id,)| user_id).collect())
}

pub async fn user_profile(pool: &PgPool, user_id: i64) -> anyhow::Result<Option<UserProfile>> {
    sqlx::query_as(
        r#"
        select username, first_name, last_name, is_bot, bio, profile_photo_file_id, profile_photo_file_unique_id
        from telegram_user_profiles where telegram_user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn chat_member_snapshot(
    pool: &PgPool,
    discussion_chat_id: i64,
    user_id: i64,
) -> anyhow::Result<Option<ChatMemberSnapshot>> {
    sqlx::query_as(
        r#"
        select status, is_admin, is_present,
               to_char(observed_at at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI') as observed_at,
               coalesce(nullif(raw_json #>> '{custom_title}', ''), nullif(raw_json #>> '{kind,custom_title}', ''),
                        nullif(raw_json #>> '{administrator,custom_title}', ''), nullif(raw_json #>> '{owner,custom_title}', ''),
                        nullif(raw_json #>> '{member,custom_title}', ''), nullif(raw_json #>> '{restricted,custom_title}', '')) as written_tag
        from telegram_chat_member_snapshots where chat_id = $1 and telegram_user_id = $2
        "#,
    )
    .bind(discussion_chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn chat_user_stats(
    pool: &PgPool,
    discussion_chat_id: i64,
    user_id: i64,
) -> anyhow::Result<Option<ChatUserStats>> {
    sqlx::query_as(
        r#"
        select to_char(first_seen_at at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI') as first_seen_at,
               to_char(last_seen_at at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI') as last_seen_at,
               first_message_id, last_message_id,
               floor(extract(epoch from (now() - first_seen_at)) / 86400)::bigint as first_seen_days_ago,
               floor(extract(epoch from (now() - last_seen_at)) / 86400)::bigint as last_seen_days_ago,
               message_count as messages, reply_count as replies, link_count as links, media_count as media,
               reply_to_channel_post_count as replies_to_channel_posts, reply_to_bot_count as replies_to_bot,
               0::bigint as voices
        from telegram_chat_users where chat_id = $1 and telegram_user_id = $2
        "#,
    )
    .bind(discussion_chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn user_totals(
    pool: &PgPool,
    discussion_chat_id: i64,
    user_id: i64,
) -> anyhow::Result<UserTotals> {
    sqlx::query_as(
        r#"
        with recursive post_thread_messages as (
            select chat_id, message_id from telegram_messages where chat_id = $1 and source_channel_id is not null
            union
            select child.chat_id, child.message_id from telegram_messages child
            join post_thread_messages parent on parent.chat_id = child.chat_id and parent.message_id = child.reply_to_message_id
            where child.chat_id = $1 and child.source_channel_id is null
        )
        select count(*)::bigint as messages, count(*) filter (where reply_to_message_id is not null)::bigint as replies,
               count(*) filter (where has_links)::bigint as links,
               count(*) filter (where has_photo or has_video or has_document or has_audio or has_voice or has_sticker or has_animation)::bigint as media,
               count(*) filter (where message_id in (select message_id from post_thread_messages))::bigint as post_comments,
               count(*) filter (where reply_to_message_id in (select bot_comment_message_id from post_comment_jobs))::bigint as replies_to_bot,
               count(distinct date_trunc('day', created_at at time zone 'Europe/Moscow' - interval '5 hours'))::bigint as active_days,
               count(*) filter (where has_voice)::bigint as voices
        from telegram_messages where chat_id = $1 and user_id = $2 and source_channel_id is null
        "#,
    )
    .bind(discussion_chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

pub async fn user_reactions_given(
    pool: &PgPool,
    discussion_chat_id: i64,
    user_id: i64,
) -> anyhow::Result<i64> {
    let (count,): (i64,) = sqlx::query_as(
        "select count(*)::bigint from telegram_message_reactions where chat_id = $1 and user_id = $2",
    )
    .bind(discussion_chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

pub async fn user_reactions_received(
    pool: &PgPool,
    discussion_chat_id: i64,
    user_id: i64,
) -> anyhow::Result<i64> {
    let (count,): (i64,) = sqlx::query_as(
        r#"select coalesce(sum(rc.total_count), 0)::bigint
           from telegram_messages m join telegram_message_reaction_counts rc on rc.chat_id = m.chat_id and rc.message_id = m.message_id
           where m.chat_id = $1 and m.user_id = $2"#,
    )
    .bind(discussion_chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

pub async fn user_top_words(
    pool: &PgPool,
    discussion_chat_id: i64,
    user_id: i64,
    stop_words: &[String],
    limit: i64,
) -> anyhow::Result<Vec<(String, i64)>> {
    sqlx::query_as(
        r#"
        select word, count(*)::bigint as usage_count
        from telegram_messages m cross join lateral regexp_split_to_table(lower(coalesce(m.text, '')), '[^[:alnum:]а-яё]+') as word
        where m.chat_id = $1 and m.user_id = $2 and char_length(word) >= 3 and word !~ '^[0-9]+$'
          and not (word = any($3::text[]))
        group by word order by usage_count desc, word asc limit $4
        "#,
    )
    .bind(discussion_chat_id)
    .bind(user_id)
    .bind(stop_words)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn resolve_user_id(
    pool: &PgPool,
    target: Option<&str>,
    reply_user_id: Option<i64>,
) -> anyhow::Result<Option<i64>> {
    let clean = target.map(clean_target_arg).unwrap_or_default();
    if clean.is_empty() {
        return Ok(reply_user_id);
    }
    if let Ok(user_id) = clean.parse::<i64>() {
        return Ok(Some(user_id));
    }

    let username = clean.trim_start_matches('@').to_lowercase();
    let row: Option<(i64,)> = sqlx::query_as(
        r#"select telegram_user_id from telegram_user_profiles
           where lower(username) = $1 order by updated_at desc limit 1"#,
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(user_id,)| user_id))
}

fn period_start_sql(period: StatsPeriod) -> &'static str {
    // The chat day is editorial, not calendar: 05:00 Moscow time is the
    // boundary for day/week/month reports.
    match period {
        StatsPeriod::Day => {
            "(date_trunc('day', now() at time zone 'Europe/Moscow' - interval '5 hours') + interval '5 hours') at time zone 'Europe/Moscow'"
        }
        StatsPeriod::Week => {
            "(date_trunc('week', now() at time zone 'Europe/Moscow' - interval '5 hours') + interval '5 hours') at time zone 'Europe/Moscow'"
        }
        StatsPeriod::Month => {
            "(date_trunc('month', now() at time zone 'Europe/Moscow' - interval '5 hours') + interval '5 hours') at time zone 'Europe/Moscow'"
        }
    }
}

fn clean_target_arg(target: &str) -> &str {
    target.trim().trim_start_matches('@')
}
