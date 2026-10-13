use sqlx::PgPool;

use crate::features::stats::types::{
    ChatStatsSummary, ReportWindow, StatsPeriod, UserModerationSummary,
};

/// Telegram's built-in service account. It is excluded from human activity rankings.
pub const TELEGRAM_SERVICE_USER_ID: i64 = 777_000;

/// Every human-activity metric uses the same population: user-authored messages
/// in the discussion chat, excluding channel-originated messages, bots, and
/// Telegram's service account. Keep `p` joined to the message author before
/// applying this SQL fragment.
const HUMAN_DISCUSSION_MESSAGE_FILTER: &str = "m.user_id is not null and m.source_channel_id is null and m.user_id <> {service_user_id} and coalesce(p.is_bot, false) = false";
const USER_ACTIVITY_MESSAGE_AGGREGATES: &str = r#"
    count(*)::bigint as messages,
    count(*) filter (where m.reply_to_message_id is not null)::bigint as replies,
    count(*) filter (where m.has_links)::bigint as links,
    count(*) filter (where m.has_photo or m.has_video or m.has_document or m.has_audio or m.has_voice or m.has_sticker or m.has_animation)::bigint as media,
    count(*) filter (where m.has_voice)::bigint as voices
"#;
const USER_ACTIVITY_REACTIONS_RECEIVED_AGGREGATE: &str =
    "coalesce(sum(rc.total_count), 0)::bigint as reactions_received";

fn with_human_message_filter(sql: &str, service_user_id_param: &str) -> String {
    sql.replace("{human_message_filter}", HUMAN_DISCUSSION_MESSAGE_FILTER)
        .replace("{service_user_id}", service_user_id_param)
}

fn with_user_activity_aggregates(sql: &str) -> String {
    sql.replace(
        "{user_activity_message_aggregates}",
        USER_ACTIVITY_MESSAGE_AGGREGATES,
    )
    .replace(
        "{user_activity_reactions_received_aggregate}",
        USER_ACTIVITY_REACTIONS_RECEIVED_AGGREGATE,
    )
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ChatStatsSummaryRow {
    start_at: chrono::DateTime<chrono::Utc>,
    messages: i64,
    active_users: i64,
    replies: i64,
    links: i64,
    media: i64,
    channel_posts: i64,
    bot_comments: i64,
    replies_to_bot: i64,
    reaction_additions: i64,
    messages_with_reaction_count_update: i64,
    bot_comment_reactions: i64,
    joins: i64,
    leaves: i64,
}

impl From<ChatStatsSummaryRow> for ChatStatsSummary {
    fn from(row: ChatStatsSummaryRow) -> Self {
        Self {
            start_at: row.start_at,
            messages: row.messages,
            active_users: row.active_users,
            replies: row.replies,
            links: row.links,
            media: row.media,
            channel_posts: row.channel_posts,
            bot_comments: row.bot_comments,
            replies_to_bot: row.replies_to_bot,
            reaction_additions: row.reaction_additions,
            messages_with_reaction_count_update: row.messages_with_reaction_count_update,
            bot_comment_reactions: row.bot_comment_reactions,
            joins: row.joins,
            leaves: row.leaves,
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AttractionMetricsRow {
    pub messages_5m: String,
    pub messages_30m: String,
    pub messages_24h: String,
    pub users_30m: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PeriodTopUserRow {
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
pub struct BotCommentStatsRow {
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
pub struct TopWordUserRow {
    pub user_id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub is_bot: bool,
    pub status: String,
    pub is_admin: bool,
    pub is_present: bool,
    pub occurrences: i64,
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

#[derive(Debug, Clone)]
pub struct UserActivityExtras {
    pub active_days: i64,
    pub reactions_given: i64,
    pub replies_to_bot: i64,
    pub post_comments: i64,
}

#[derive(Debug, Clone)]
pub struct UserActivitySummary {
    pub activity: teloxide_statistics::activity::UserActivity,
    pub extras: UserActivityExtras,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct UserActivityCountsRow {
    messages: i64,
    replies: i64,
    links: i64,
    media: i64,
    voices: i64,
    active_days: i64,
    reactions_given: i64,
    reactions_received: i64,
    replies_to_bot: i64,
}

pub async fn user_moderation_summary(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
) -> anyhow::Result<UserModerationSummary> {
    #[derive(sqlx::FromRow)]
    struct Row {
        active_restriction: Option<String>,
        restriction_expires_at: Option<chrono::DateTime<chrono::Utc>>,
        active_warnings: i64,
        unknown_restriction: bool,
    }

    let row: Row = sqlx::query_as(
        r#"select restriction.action as active_restriction,
                  restriction.expires_at as restriction_expires_at,
                  (select count(*) from manual_moderation_actions warning
                   where warning.chat_id = $1 and warning.target_user_id = $2
                     and warning.action = 'warn' and warning.status = 'applied'
                     and (warning.expires_at is null or warning.expires_at > now())) as active_warnings,
                  exists (
                    select 1 from manual_moderation_actions uncertain
                    where uncertain.chat_id = $1 and uncertain.target_user_id = $2
                      and uncertain.action in ('mute', 'ban', 'auto_mute')
                      and uncertain.status = 'unknown'
                      and (uncertain.expires_at is null or uncertain.expires_at > now())
                  ) as unknown_restriction
           from (select 1) seed
           left join lateral (
             select action, expires_at from manual_moderation_actions
             where chat_id = $1 and target_user_id = $2 and action in ('mute', 'ban', 'auto_mute')
               and status = 'applied' and (expires_at is null or expires_at > now())
             order by created_at desc, id desc limit 1
           ) restriction on true"#,
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(UserModerationSummary {
        active_restriction: row.active_restriction,
        restriction_expires_at: row.restriction_expires_at,
        active_warnings: row.active_warnings,
        unknown_restriction: row.unknown_restriction,
    })
}

/// Resolves the editorial period once so all report queries use identical bounds.
pub async fn report_window(pool: &PgPool, period: StatsPeriod) -> anyhow::Result<ReportWindow> {
    #[derive(sqlx::FromRow)]
    struct ReportWindowRow {
        start_at: chrono::DateTime<chrono::Utc>,
        end_at: chrono::DateTime<chrono::Utc>,
        chat_day_index: i64,
    }

    let sql = format!(
        "select {} as start_at, now() as end_at, \
         (date_trunc('day', now() at time zone 'Europe/Moscow' - interval '5 hours')::date - date '1970-01-01')::bigint as chat_day_index",
        period_start_sql(period)
    );
    let row: ReportWindowRow = sqlx::query_as(&sql).fetch_one(pool).await?;
    Ok(ReportWindow::new(
        row.start_at,
        row.end_at,
        row.chat_day_index,
    ))
}

pub async fn chat_stats_summary(
    pool: &PgPool,
    discussion_chat_id: i64,
    window: ReportWindow,
) -> anyhow::Result<ChatStatsSummary> {
    let sql = r#"
        with bounds as (
            select $3::timestamptz as start_at, $4::timestamptz as end_at
        ),
        messages as (
            select m.* from telegram_messages m, bounds b
            where m.chat_id = $1 and m.created_at >= b.start_at and m.created_at < b.end_at
        ),
        human_messages as (
            select m.*
            from messages m
            left join telegram_user_profiles p on p.telegram_user_id = m.user_id
            where {human_message_filter}
        ),
        bot_comments as (
            select j.* from post_comment_jobs j, bounds b
            where j.discussion_chat_id = $1 and j.status = 'sent'
              and j.sent_at >= b.start_at and j.sent_at < b.end_at
        ),
        reactions as (
            select r.* from telegram_message_reactions r, bounds b
            where r.chat_id = $1 and r.event_at >= b.start_at and r.event_at < b.end_at
        ),
        reaction_counts as (
            select rc.* from telegram_message_reaction_counts rc, bounds b
            where rc.chat_id = $1 and rc.event_at >= b.start_at and rc.event_at < b.end_at
        ),
        bot_comment_reactions as (
            select coalesce(sum(rc.total_count), 0)::bigint as total
            from bot_comments j
            left join telegram_message_reaction_counts rc
              on rc.chat_id = j.discussion_chat_id
             and rc.message_id = j.bot_comment_message_id
        ),
        member_events as (
            select e.* from telegram_chat_member_events e, bounds b
            where e.chat_id = $1 and e.event_at >= b.start_at and e.event_at < b.end_at
        )
        select
            (select start_at from bounds) as start_at,
            {user_activity_message_aggregates},
            count(distinct m.user_id)::bigint as active_users,
            (select count(*) from messages where is_automatic_forward)::bigint as channel_posts,
            (select count(*) from bot_comments)::bigint as bot_comments,
            (select count(*) from human_messages m join bot_comments j on m.reply_to_message_id = j.bot_comment_message_id)::bigint as replies_to_bot,
            (select count(*)
             from reactions r,
                  jsonb_array_elements(r.new_reactions) as added(value)
             where not exists (
                 select 1 from jsonb_array_elements(r.old_reactions) as previous(value)
                 where previous.value = added.value
             ))::bigint as reaction_additions,
            (select count(*) from reaction_counts)::bigint as messages_with_reaction_count_update,
            (select total from bot_comment_reactions)::bigint as bot_comment_reactions,
            (select count(*) from member_events where old_status in ('left', 'banned') and new_status not in ('left', 'banned'))::bigint as joins,
            (select count(*) from member_events where old_status not in ('left', 'banned') and new_status in ('left', 'banned'))::bigint as leaves
        from human_messages m
        "#;
    let sql = with_user_activity_aggregates(&with_human_message_filter(sql, "$2"));

    let row: ChatStatsSummaryRow = sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .bind(TELEGRAM_SERVICE_USER_ID)
        .bind(window.start_at)
        .bind(window.end_at)
        .fetch_one(pool)
        .await?;
    Ok(row.into())
}

pub async fn chat_attraction_metrics(
    pool: &PgPool,
    discussion_chat_id: i64,
    window: ReportWindow,
) -> anyhow::Result<AttractionMetricsRow> {
    // Cohorts start when Telegram confirms a comment was sent. A window contributes
    // only after it is complete at the fixed report boundary; otherwise `-` avoids
    // presenting a partial cohort as a zero-engagement result.
    let sql = r#"
        with bounds as (select $3::timestamptz as start_at, $4::timestamptz as end_at),
        human_messages as (
            select m.*
            from telegram_messages m
            left join telegram_user_profiles p on p.telegram_user_id = m.user_id
            where m.chat_id = $1 and {human_message_filter}
        ),
        metrics as (
            select j.sent_at,
                   count(m.*) filter (where m.created_at <= j.sent_at + interval '5 minutes' and coalesce(m.text,'') !~ '^/') as messages_5m,
                   count(m.*) filter (where m.created_at <= j.sent_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/') as messages_30m,
                   count(m.*) filter (where m.created_at <= j.sent_at + interval '24 hours' and coalesce(m.text,'') !~ '^/') as messages_24h,
                   count(distinct m.user_id) filter (where m.created_at <= j.sent_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/') as users_30m
            from post_comment_jobs j
            left join human_messages m on m.chat_id = j.discussion_chat_id
                and m.created_at > j.sent_at and m.created_at <= j.sent_at + interval '24 hours'
                and m.message_id <> j.bot_comment_message_id
            where j.discussion_chat_id = $1 and j.status = 'sent'
              and j.sent_at >= (select start_at from bounds) and j.sent_at < (select end_at from bounds)
            group by j.sent_at, j.bot_comment_message_id
        )
        select coalesce(round((avg(messages_5m) filter (where sent_at + interval '5 minutes' <= (select end_at from bounds)))::numeric, 2)::text, '-') as messages_5m,
               coalesce(round((avg(messages_30m) filter (where sent_at + interval '30 minutes' <= (select end_at from bounds)))::numeric, 2)::text, '-') as messages_30m,
               coalesce(round((avg(messages_24h) filter (where sent_at + interval '24 hours' <= (select end_at from bounds)))::numeric, 2)::text, '-') as messages_24h,
               coalesce(round((avg(users_30m) filter (where sent_at + interval '30 minutes' <= (select end_at from bounds)))::numeric, 2)::text, '-') as users_30m
        from metrics
        "#;
    let sql = with_human_message_filter(sql, "$2");

    sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .bind(TELEGRAM_SERVICE_USER_ID)
        .bind(window.start_at)
        .bind(window.end_at)
        .fetch_one(pool)
        .await
        .map_err(Into::into)
}

pub async fn period_top_users(
    pool: &PgPool,
    discussion_chat_id: i64,
    window: ReportWindow,
    limit: i64,
) -> anyhow::Result<Vec<PeriodTopUserRow>> {
    let sql = r#"
        with bounds as (select $3::timestamptz as start_at, $4::timestamptz as end_at)
        select m.user_id, p.username, p.first_name, p.last_name,
               {user_activity_message_aggregates},
               coalesce(s.status, 'unknown') as status, coalesce(s.is_admin, false) as is_admin,
               coalesce(s.is_present, false) as is_present
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = m.chat_id and s.telegram_user_id = m.user_id
        where m.chat_id = $1 and {human_message_filter}
          and m.created_at >= (select start_at from bounds) and m.created_at < (select end_at from bounds)
        group by m.user_id, p.username, p.first_name, p.last_name, s.status, s.is_admin, s.is_present
        order by messages desc
        limit $5
        "#;
    let sql = with_user_activity_aggregates(&with_human_message_filter(sql, "$2"));

    sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .bind(TELEGRAM_SERVICE_USER_ID)
        .bind(window.start_at)
        .bind(window.end_at)
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(Into::into)
}

pub async fn bot_comments_for_period(
    pool: &PgPool,
    discussion_chat_id: i64,
    window: ReportWindow,
    limit: i64,
) -> anyhow::Result<Vec<BotCommentStatsRow>> {
    let sql = r#"
        with bounds as (select $3::timestamptz as start_at, $4::timestamptz as end_at),
        human_messages as (
            select m.*
            from telegram_messages m
            left join telegram_user_profiles p on p.telegram_user_id = m.user_id
            where m.chat_id = $1 and {human_message_filter}
        )
        select j.source_message_id, coalesce(g.response, '') as response,
               count(m.*) filter (where m.created_at <= j.sent_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/')::bigint as messages_30m,
               count(m.*) filter (where m.reply_to_message_id = j.bot_comment_message_id)::bigint as direct_replies,
               coalesce(max(rc.total_count), 0)::bigint as reactions
        from post_comment_jobs j
        left join llm_generations g on g.post_comment_job_id = j.id
        left join human_messages m on m.chat_id = j.discussion_chat_id
          and m.created_at > j.sent_at and m.created_at <= j.sent_at + interval '30 minutes'
            and m.message_id <> j.bot_comment_message_id
        left join telegram_message_reaction_counts rc on rc.chat_id = j.discussion_chat_id and rc.message_id = j.bot_comment_message_id
        where j.discussion_chat_id = $1 and j.status = 'sent'
          and j.sent_at >= (select start_at from bounds) and j.sent_at < (select end_at from bounds)
        group by j.source_message_id, g.response, j.sent_at
        order by messages_30m desc, direct_replies desc, reactions desc
        limit $5
        "#;
    let sql = with_human_message_filter(sql, "$2");

    sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .bind(TELEGRAM_SERVICE_USER_ID)
        .bind(window.start_at)
        .bind(window.end_at)
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
    ranked_message_users(pool, discussion_chat_id, limit, false).await
}

pub async fn bottom_message_users(
    pool: &PgPool,
    discussion_chat_id: i64,
    limit: i64,
) -> anyhow::Result<Vec<TopMessage>> {
    ranked_message_users(pool, discussion_chat_id, limit, true).await
}

pub async fn top_word_users(
    pool: &PgPool,
    discussion_chat_id: i64,
    word: &str,
    limit: i64,
) -> anyhow::Result<Vec<TopWordUserRow>> {
    let sql = r#"
        with eligible_messages as (
            select m.user_id, m.text
            from telegram_messages m
            left join telegram_user_profiles p on p.telegram_user_id = m.user_id
            where m.chat_id = $1 and {human_message_filter}
              and lower(coalesce(m.text, '')) like '%' || lower($3) || '%'
        ), word_counts as (
            select m.user_id, count(*)::bigint as occurrences
            from eligible_messages m
            cross join lateral regexp_split_to_table(
                lower(coalesce(m.text, '')), '[^[:alnum:]а-яё]+'
            ) as tokens(token)
            where tokens.token = lower($3)
            group by m.user_id
        )
        select counts.user_id, p.username,
               coalesce(nullif(case when p.first_name = 'пользователь' then '' else p.first_name end, ''), raw_name.display_name, 'скрытый пользователь') as first_name,
               p.last_name, coalesce(p.is_bot, false) as is_bot, coalesce(s.status, 'unknown') as status,
               coalesce(s.is_admin, false) as is_admin, coalesce(s.is_present, false) as is_present,
               counts.occurrences
        from word_counts counts
        left join telegram_user_profiles p on p.telegram_user_id = counts.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = $1 and s.telegram_user_id = counts.user_id
        left join lateral (
            select coalesce(nullif(tm.raw_json #>> '{from,first_name}', ''), nullif(tm.raw_json ->> 'from', '')) as display_name
            from telegram_messages tm
            where tm.chat_id = $1 and tm.user_id = counts.user_id
              and coalesce(nullif(tm.raw_json #>> '{from,first_name}', ''), nullif(tm.raw_json ->> 'from', '')) is not null
            order by tm.created_at desc limit 1
        ) raw_name on true
        order by counts.occurrences desc, counts.user_id asc
        limit $4
        "#;
    let sql = with_human_message_filter(sql, "$2");

    sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .bind(TELEGRAM_SERVICE_USER_ID)
        .bind(word)
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(Into::into)
}

async fn ranked_message_users(
    pool: &PgPool,
    discussion_chat_id: i64,
    limit: i64,
    ascending: bool,
) -> anyhow::Result<Vec<TopMessage>> {
    let order = if ascending {
        "messages asc, reactions_received asc"
    } else {
        "messages desc, reactions_received desc"
    };
    let message_aggregates = USER_ACTIVITY_MESSAGE_AGGREGATES;
    let reactions_received_aggregate = USER_ACTIVITY_REACTIONS_RECEIVED_AGGREGATE;
    let sql = format!(
        r#"
        select m.user_id, p.username,
               coalesce(nullif(case when p.first_name = 'пользователь' then '' else p.first_name end, ''), raw_name.display_name, 'скрытый пользователь') as first_name,
               p.last_name, coalesce(p.is_bot, false) as is_bot, coalesce(s.status, 'unknown') as status,
               coalesce(s.is_admin, false) as is_admin, coalesce(s.is_present, false) as is_present,
               {message_aggregates},
               {reactions_received_aggregate}
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = m.chat_id and s.telegram_user_id = m.user_id
        left join telegram_message_reaction_counts rc on rc.chat_id = m.chat_id and rc.message_id = m.message_id
        left join lateral (
            select coalesce(nullif(tm.raw_json #>> '{{from,first_name}}', ''), nullif(tm.raw_json ->> 'from', '')) as display_name
            from telegram_messages tm
            where tm.chat_id = m.chat_id and tm.user_id = m.user_id
              and coalesce(nullif(tm.raw_json #>> '{{from,first_name}}', ''), nullif(tm.raw_json ->> 'from', '')) is not null
            order by tm.created_at desc limit 1
        ) raw_name on true
        where m.chat_id = $1 and {{human_message_filter}}
        group by m.user_id, p.username, p.first_name, p.last_name, p.is_bot, s.status, s.is_admin, s.is_present, raw_name.display_name
        order by {order}
        limit $3
        "#,
    );
    let sql = with_human_message_filter(&sql, "$2");

    sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .bind(TELEGRAM_SERVICE_USER_ID)
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(Into::into)
}

/// Daily active users for a chat window: `(chat_day, users)` ordered by day.
/// The reporting day follows the same 05:00 Moscow boundary as period reports.
pub async fn daily_active_users(
    pool: &PgPool,
    discussion_chat_id: i64,
    window: ReportWindow,
) -> anyhow::Result<Vec<(chrono::NaiveDate, i64)>> {
    #[derive(sqlx::FromRow)]
    struct DayRow {
        day: chrono::NaiveDate,
        users: i64,
    }

    let sql = r#"
        with bounds as (select $3::timestamptz as start_at, $4::timestamptz as end_at)
        select date_trunc('day', m.created_at at time zone 'Europe/Moscow' - interval '5 hours')::date as day,
               count(distinct m.user_id)::bigint as users
        from telegram_messages m
        cross join bounds b
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1 and {human_message_filter}
          and m.created_at >= b.start_at and m.created_at < b.end_at
        group by 1 order by 1
        "#;
    let sql = with_human_message_filter(sql, "$2");

    let rows: Vec<DayRow> = sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .bind(TELEGRAM_SERVICE_USER_ID)
        .bind(window.start_at)
        .bind(window.end_at)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|row| (row.day, row.users)).collect())
}

/// First-seen day plus active days per user over the trailing lookback window.
/// Feeds `teloxide_statistics::engagement::retention_table`; day indices use
/// the same 05:00 Moscow reporting boundary as `/userstatus` active days.
pub async fn cohort_activity(
    pool: &PgPool,
    discussion_chat_id: i64,
    since: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<Vec<(i64, i64)>> {
    #[derive(sqlx::FromRow)]
    struct ActivityRow {
        user_id: i64,
        day: i64,
    }

    let sql = r#"
        with eligible_users as (
            select m.user_id from telegram_messages m
            left join telegram_user_profiles p on p.telegram_user_id = m.user_id
            where m.chat_id = $1 and {human_message_filter}
            group by m.user_id having min(m.created_at) >= $3
        )
        select m.user_id,
               (date_trunc('day', m.created_at at time zone 'Europe/Moscow' - interval '5 hours')::date
                - date '1970-01-01')::bigint as day
        from telegram_messages m
        join eligible_users u on u.user_id = m.user_id
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1 and {human_message_filter} and m.created_at >= $3
        group by m.user_id, 2
        "#;
    let sql = with_human_message_filter(sql, "$2");

    let rows: Vec<ActivityRow> = sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .bind(TELEGRAM_SERVICE_USER_ID)
        .bind(since)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|row| (row.user_id, row.day)).collect())
}

/// Added reaction identities in the window, grouped by visible emoji. Callers
/// classify with `teloxide_statistics::sentiment`; custom emoji have no glyph
/// and are kept under an empty key so they contribute to `Unknown`.
pub async fn period_reaction_emoji_counts(
    pool: &PgPool,
    discussion_chat_id: i64,
    window: ReportWindow,
) -> anyhow::Result<Vec<(String, i64)>> {
    #[derive(sqlx::FromRow)]
    struct EmojiCountRow {
        emoji: String,
        count: i64,
    }

    let rows: Vec<EmojiCountRow> = sqlx::query_as(
        r#"
        with bounds as (select $2::timestamptz as start_at, $3::timestamptz as end_at)
        select coalesce(added.value ->> 'emoji', '') as emoji, count(*)::bigint as count
        from telegram_message_reactions r, bounds b,
             jsonb_array_elements(r.new_reactions) as added(value)
        where r.chat_id = $1 and r.event_at >= b.start_at and r.event_at < b.end_at
          and not exists (
              select 1 from jsonb_array_elements(r.old_reactions) as previous(value)
              where previous.value = added.value
          )
        group by coalesce(added.value ->> 'emoji', '')
        "#,
    )
    .bind(discussion_chat_id)
    .bind(window.start_at)
    .bind(window.end_at)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|row| (row.emoji, row.count)).collect())
}

pub async fn top_reacted_messages(
    pool: &PgPool,
    discussion_chat_id: i64,
    limit: i64,
) -> anyhow::Result<Vec<TopReactedMessage>> {
    let sql = r#"
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
        where rc.chat_id = $1 and rc.total_count > 0 and {human_message_filter}
        order by rc.total_count desc, m.created_at desc
        limit $3
        "#;
    let sql = with_human_message_filter(sql, "$2");

    sqlx::query_as(&sql)
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
    let sql = r#"
        select m.user_id from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1 and {human_message_filter}
        group by m.user_id order by count(*) desc limit $3
        "#;
    let sql = with_human_message_filter(sql, "$2");

    let rows: Vec<(i64,)> = sqlx::query_as(&sql)
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
    let sql = r#"
        select m.user_id from telegram_message_reaction_counts rc
        join telegram_messages m on m.chat_id = rc.chat_id and m.message_id = rc.message_id
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where rc.chat_id = $1 and rc.total_count > 0 and {human_message_filter}
        group by m.user_id order by max(rc.total_count) desc limit $3
        "#;
    let sql = with_human_message_filter(sql, "$2");

    let rows: Vec<(i64,)> = sqlx::query_as(&sql)
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
        select username, first_name, last_name, is_bot, bio, profile_photo_file_id,
            profile_photo_file_unique_id
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

pub async fn user_activity(
    pool: &PgPool,
    discussion_chat_id: i64,
    user_id: i64,
) -> anyhow::Result<UserActivitySummary> {
    let sql = r#"
        with human_messages as (
            select m.*
            from telegram_messages m
            left join telegram_user_profiles p on p.telegram_user_id = m.user_id
            where m.chat_id = $1 and m.user_id = $2
              and {human_message_filter}
        ),
        reactions_given as (
            select count(*)::bigint as total
            from telegram_message_reactions r
            cross join lateral jsonb_array_elements(r.new_reactions) as added(value)
            where r.chat_id = $1 and r.user_id = $2
              and not exists (
                  select 1 from jsonb_array_elements(r.old_reactions) as previous(value)
                  where previous.value = added.value
              )
              and $2 <> $3
              and coalesce((
                  select p.is_bot from telegram_user_profiles p
                  where p.telegram_user_id = $2 limit 1
              ), false) = false
        ),
        message_counts as (
            select {user_activity_message_aggregates},
                   count(distinct date_trunc('day', m.created_at at time zone 'Europe/Moscow' - interval '5 hours'))::bigint as active_days,
                   count(*) filter (where m.reply_to_message_id in (
                       select bot_comment_message_id from post_comment_jobs
                       where discussion_chat_id = $1 and status = 'sent'
                   ))::bigint as replies_to_bot,
                   {user_activity_reactions_received_aggregate}
            from human_messages m
            left join telegram_message_reaction_counts rc
              on rc.chat_id = m.chat_id and rc.message_id = m.message_id
        )
        select mc.messages, mc.replies, mc.links, mc.media, mc.voices, mc.active_days,
               (select total from reactions_given) as reactions_given,
               mc.reactions_received,
               mc.replies_to_bot
        from message_counts mc
        "#;
    let sql = with_user_activity_aggregates(&with_human_message_filter(sql, "$3"));
    let row: UserActivityCountsRow = sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .bind(user_id)
        .bind(TELEGRAM_SERVICE_USER_ID)
        .fetch_one(pool)
        .await?;

    Ok(UserActivitySummary {
        activity: teloxide_statistics::activity::UserActivity {
            messages: row.messages,
            replies: row.replies,
            links: row.links,
            media: row.media,
            voices: row.voices,
            reactions_received: row.reactions_received,
        },
        extras: UserActivityExtras {
            active_days: row.active_days,
            reactions_given: row.reactions_given,
            replies_to_bot: row.replies_to_bot,
            post_comments: 0,
        },
    })
}

pub async fn user_post_comment_count(
    pool: &PgPool,
    discussion_chat_id: i64,
    user_id: i64,
) -> anyhow::Result<i64> {
    let (count,): (i64,) = sqlx::query_as(
        r#"
        with recursive post_thread_messages as (
            select chat_id, message_id
            from telegram_messages
            where chat_id = $1 and source_channel_id is not null

            union

            select child.chat_id, child.message_id
            from telegram_messages child
            join post_thread_messages parent
              on parent.chat_id = child.chat_id
             and parent.message_id = child.reply_to_message_id
            where child.chat_id = $1 and child.source_channel_id is null
        )
        select count(*)::bigint
        from telegram_messages messages
        where messages.chat_id = $1
          and messages.user_id = $2
          and messages.source_channel_id is null
          and messages.user_id <> $3
          and not exists (
              select 1 from telegram_user_profiles p
              where p.telegram_user_id = messages.user_id and p.is_bot
          )
          and messages.message_id in (
              select message_id from post_thread_messages
          )
        "#,
    )
    .bind(discussion_chat_id)
    .bind(user_id)
    .bind(TELEGRAM_SERVICE_USER_ID)
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
    let sql = r#"
        select word, count(*)::bigint as usage_count
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        cross join lateral regexp_split_to_table(lower(coalesce(m.text, '')), '[^[:alnum:]а-яё]+') as word
        where m.chat_id = $1 and m.user_id = $2 and {human_message_filter}
          and char_length(word) >= 3 and word !~ '^[0-9]+$'
          and not (word = any($3::text[]))
        group by word order by usage_count desc, word asc limit $4
        "#;
    let sql = with_human_message_filter(sql, "$5");

    sqlx::query_as(&sql)
        .bind(discussion_chat_id)
        .bind(user_id)
        .bind(stop_words)
        .bind(limit)
        .bind(TELEGRAM_SERVICE_USER_ID)
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
