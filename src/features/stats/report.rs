use sqlx::PgPool;
use teloxide::prelude::*;

use crate::config::Config;
use crate::features::stats::types::{
    ChatStatsSummary, StatsPeriod, UserPresentation, display_name,
};
use crate::telegram::html::{Html, truncate_text};
use crate::telegram::render::{escape_html, send_html};
use crate::text::normalize_ai_markers;

pub async fn send_chat_stats(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> ResponseResult<()> {
    let report = build_chat_stats_report(pool, config, period)
        .await
        .map_err(|err| {
            tracing::error!(%err, "failed to build chat stats");
            teloxide::RequestError::Io(std::io::Error::other("stats failed"))
        })?;

    send_html(bot, chat_id, report).await?;

    Ok(())
}

pub async fn send_user_stats(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    target: &str,
) -> ResponseResult<()> {
    let report = build_user_stats_report(pool, config, target)
        .await
        .map_err(|err| {
            tracing::error!(%err, "failed to build user stats");
            teloxide::RequestError::Io(std::io::Error::other("user stats failed"))
        })?;

    send_html(bot, chat_id, report).await?;

    Ok(())
}

async fn build_chat_stats_report(
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> anyhow::Result<String> {
    let start_sql = period.start_sql();
    let summary_sql = format!(
        r#"
        with bounds as (
            select {start_sql} as start_at, now() as end_at
        ),
        messages as (
            select m.*
            from telegram_messages m, bounds b
            where m.chat_id = $1
              and m.created_at >= b.start_at
              and m.created_at < b.end_at
        ),
        bot_comments as (
            select j.*
            from post_comment_jobs j, bounds b
            where j.discussion_chat_id = $1
              and j.created_at >= b.start_at
              and j.created_at < b.end_at
        ),
        reactions as (
            select r.*
            from telegram_message_reactions r, bounds b
            where r.chat_id = $1
              and r.event_at >= b.start_at
              and r.event_at < b.end_at
        ),
        reaction_counts as (
            select rc.*
            from telegram_message_reaction_counts rc, bounds b
            where rc.chat_id = $1
              and rc.event_at >= b.start_at
              and rc.event_at < b.end_at
        ),
        member_events as (
            select e.*
            from telegram_chat_member_events e, bounds b
            where e.chat_id = $1
              and e.event_at >= b.start_at
              and e.event_at < b.end_at
        )
        select
            to_char((select start_at from bounds) at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI') as start_label,
            count(*)::bigint as messages,
            count(distinct user_id) filter (where source_channel_id is null and coalesce(user_id, 0) <> 777000)::bigint as active_users,
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
        "#
    );

    let summary: ChatStatsSummary = sqlx::query_as(&summary_sql)
        .bind(config.discussion_chat_id)
        .fetch_one(pool)
        .await?;

    let attraction_sql = format!(
        r#"
        with bounds as (
            select {start_sql} as start_at, now() as end_at
        ),
        metrics as (
            select j.source_message_id,
                   count(m.*) filter (where m.created_at <= j.created_at + interval '5 minutes' and coalesce(m.text,'') !~ '^/') as msg_5m,
                   count(m.*) filter (where m.created_at <= j.created_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/') as msg_30m,
                   count(distinct m.user_id) filter (where m.created_at <= j.created_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/') as users_30m
            from post_comment_jobs j
            left join telegram_messages m
              on m.chat_id = j.discussion_chat_id
             and m.created_at > j.created_at
             and m.created_at <= j.created_at + interval '30 minutes'
             and m.message_id <> j.bot_comment_message_id
             and m.source_channel_id is null
            where j.discussion_chat_id = $1
              and j.created_at >= (select start_at from bounds)
              and j.created_at < (select end_at from bounds)
            group by j.source_message_id, j.created_at, j.bot_comment_message_id
        )
        select
            coalesce(round(avg(msg_5m)::numeric, 2), 0)::text,
            coalesce(round(avg(msg_30m)::numeric, 2), 0)::text,
            coalesce(round(avg(users_30m)::numeric, 2), 0)::text
        from metrics
        "#
    );
    let attraction: (String, String, String) = sqlx::query_as(&attraction_sql)
        .bind(config.discussion_chat_id)
        .fetch_one(pool)
        .await?;

    let top_users = top_users_for_period(pool, config, period).await?;
    let top_bot_comments = top_bot_comments_for_period(pool, config, period).await?;

    let mut report = format!(
        "<b>Статистика за {}</b>\nПериод с <code>{}</code> МСК\n\nСообщения: <b>{}</b>\nАктивных пользователей: <b>{}</b>\nРеплаи: <b>{}</b>, ссылки: <b>{}</b>, медиа: <b>{}</b>\nПосты канала: <b>{}</b>, комменты бота: <b>{}</b>\nРеплаи на бота: <b>{}</b>\nРеакции events: <b>{}</b>, count updates: <b>{}</b>\nРеакции на комменты бота: <b>{}</b>\nВходы: <b>{}</b>, выходы: <b>{}</b>\n\nЗавлечение после коммента: 5м <b>{}</b>, 30м <b>{}</b>, людей 30м <b>{}</b>",
        period.title(),
        escape_html(&summary.start_label),
        summary.messages,
        summary.active_users,
        summary.replies,
        summary.links,
        summary.media,
        summary.channel_posts,
        summary.bot_comments,
        summary.replies_to_bot,
        summary.reaction_events,
        summary.reaction_count_updates,
        summary.bot_comment_reactions,
        summary.joins,
        summary.leaves,
        attraction.0,
        attraction.1,
        attraction.2,
    );

    if !top_users.is_empty() {
        report.push_str("\n\n<b>Топ пользователей</b>\n");
        report.push_str(&top_users.join("\n"));
    }

    if !top_bot_comments.is_empty() {
        report.push_str("\n\n<b>Комменты бота</b>\n");
        report.push_str(&top_bot_comments.join("\n"));
    }

    Ok(report)
}

async fn top_users_for_period(
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> anyhow::Result<Vec<String>> {
    let sql = format!(
        r#"
        with bounds as (
            select {} as start_at, now() as end_at
        )
        select m.user_id,
               p.username,
               p.first_name,
               p.last_name,
               count(*)::bigint as messages,
               count(*) filter (where m.reply_to_message_id is not null)::bigint as replies,
               count(*) filter (where m.has_links)::bigint as links,
               count(*) filter (where m.has_photo or m.has_video or m.has_document or m.has_audio or m.has_voice or m.has_sticker or m.has_animation)::bigint as media,
               coalesce(s.status, 'unknown') as status,
               coalesce(s.is_admin, false) as is_admin,
               coalesce(s.is_present, false) as is_present
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = m.chat_id and s.telegram_user_id = m.user_id
        where m.chat_id = $1
          and m.user_id is not null
          and m.source_channel_id is null
          and m.user_id <> 777000
          and coalesce(p.is_bot, false) = false
          and m.created_at >= (select start_at from bounds)
          and m.created_at < (select end_at from bounds)
        group by m.user_id, p.username, p.first_name, p.last_name, s.status, s.is_admin, s.is_present
        order by messages desc
        limit 8
        "#,
        period.start_sql()
    );

    let rows = sqlx::query_as::<
        _,
        (
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
            i64,
            i64,
            String,
            bool,
            bool,
        ),
    >(&sql)
    .bind(config.discussion_chat_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                user_id,
                username,
                first_name,
                last_name,
                messages,
                replies,
                links,
                media,
                status,
                is_admin,
                is_present,
            )| {
                let user = UserPresentation {
                    user_id,
                    display_name: display_name(
                        username.as_deref(),
                        first_name.as_deref(),
                        last_name.as_deref(),
                        user_id,
                    ),
                    is_bot: false,
                    status: Some(status),
                    is_admin,
                    is_present: Some(is_present),
                };
                format!(
                    "{}: <b>{}</b> соо, {} реплаев, {} ссылок, {} медиа",
                    user.linked_with_badges(),
                    messages,
                    replies,
                    links,
                    media
                )
            },
        )
        .collect())
}

async fn top_bot_comments_for_period(
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> anyhow::Result<Vec<String>> {
    let sql = format!(
        r#"
        with bounds as (
            select {} as start_at, now() as end_at
        )
        select j.source_message_id,
               coalesce(g.response, '') as response,
               count(m.*) filter (where m.created_at <= j.created_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/')::bigint as msg_30m,
               count(m.*) filter (where m.reply_to_message_id = j.bot_comment_message_id)::bigint as direct_replies,
               coalesce(max(rc.total_count), 0)::bigint as reactions
        from post_comment_jobs j
        left join llm_generations g on g.post_comment_job_id = j.id
        left join telegram_messages m
          on m.chat_id = j.discussion_chat_id
         and m.created_at > j.created_at
         and m.created_at <= j.created_at + interval '30 minutes'
         and m.message_id <> j.bot_comment_message_id
         and m.source_channel_id is null
        left join telegram_message_reaction_counts rc
          on rc.chat_id = j.discussion_chat_id
         and rc.message_id = j.bot_comment_message_id
        where j.discussion_chat_id = $1
          and j.created_at >= (select start_at from bounds)
          and j.created_at < (select end_at from bounds)
        group by j.source_message_id, g.response
        order by msg_30m desc, direct_replies desc, reactions desc
        limit 5
        "#,
        period.start_sql()
    );

    let rows = sqlx::query_as::<_, (i32, String, i64, i64, i64)>(&sql)
        .bind(config.discussion_chat_id)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(
            |(source_message_id, response, msg_30m, direct_replies, reactions)| {
                let clean_response = human_comment_preview(&response);
                format!(
                    "#{}: {} соо за 30м, {} реплаев, {} реакций - {}",
                    source_message_id,
                    msg_30m,
                    direct_replies,
                    reactions,
                    Html::text(truncate_text(&clean_response, 110)).into_string()
                )
            },
        )
        .collect())
}

fn human_comment_preview(text: &str) -> String {
    normalize_ai_markers(text)
        .replace("{CHAT_LINK}", "чат")
        .replace("  ", " ")
        .trim()
        .to_string()
}

async fn build_user_stats_report(
    pool: &PgPool,
    config: &Config,
    target: &str,
) -> anyhow::Result<String> {
    let Some(user_id) = resolve_user_id(pool, target).await? else {
        return Ok(format!(
            "Не нашёл пользователя {}. Используй id или @username из уже виденных ботом пользователей.",
            Html::code(target).into_string()
        ));
    };

    let profile = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, bool)>(
        r#"
        select username, first_name, last_name, is_bot
        from telegram_user_profiles
        where telegram_user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let member = sqlx::query_as::<_, (String, bool, bool, Option<String>)>(
        r#"
        select status, is_admin, is_present, to_char(observed_at at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI')
        from telegram_chat_member_snapshots
        where chat_id = $1 and telegram_user_id = $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let user_data = sqlx::query_as::<
        _,
        (
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<i32>,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
        ),
    >(
        r#"
        select to_char(first_seen_at at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI'),
               to_char(last_seen_at at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI'),
               first_message_id,
               last_message_id,
               message_count,
               reply_count,
               link_count,
               media_count,
               reply_to_channel_post_count,
               reply_to_bot_count
        from telegram_chat_users
        where chat_id = $1 and telegram_user_id = $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let totals = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64)>(
        r#"
        select count(*)::bigint as messages,
               count(*) filter (where reply_to_message_id is not null)::bigint as replies,
               count(*) filter (where has_links)::bigint as links,
               count(*) filter (where has_photo or has_video or has_document or has_audio or has_voice or has_sticker or has_animation)::bigint as media,
               count(*) filter (where reply_to_message_id in (select discussion_message_id from post_comment_jobs))::bigint as replies_to_channel_posts,
               count(*) filter (where reply_to_message_id in (select bot_comment_message_id from post_comment_jobs))::bigint as replies_to_bot,
               count(distinct date_trunc('day', created_at at time zone 'Europe/Moscow' - interval '5 hours'))::bigint as active_days
        from telegram_messages
        where chat_id = $1 and user_id = $2 and source_channel_id is null
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    let reactions_given = sqlx::query_as::<_, (i64,)>(
        r#"
        select count(*)::bigint
        from telegram_message_reactions
        where chat_id = $1 and user_id = $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?
    .0;

    let reactions_received = sqlx::query_as::<_, (i64,)>(
        r#"
        select coalesce(sum(rc.total_count), 0)::bigint
        from telegram_messages m
        join telegram_message_reaction_counts rc on rc.chat_id = m.chat_id and rc.message_id = m.message_id
        where m.chat_id = $1 and m.user_id = $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?
    .0;

    let user = match (profile.as_ref(), member.as_ref()) {
        (
            Some((username, first_name, last_name, is_bot)),
            Some((status, is_admin, is_present, _)),
        ) => UserPresentation {
            user_id,
            display_name: display_name(
                username.as_deref(),
                first_name.as_deref(),
                last_name.as_deref(),
                user_id,
            ),
            is_bot: *is_bot,
            status: Some(status.clone()),
            is_admin: *is_admin,
            is_present: Some(*is_present),
        },
        (Some((username, first_name, last_name, is_bot)), None) => UserPresentation {
            user_id,
            display_name: display_name(
                username.as_deref(),
                first_name.as_deref(),
                last_name.as_deref(),
                user_id,
            ),
            is_bot: *is_bot,
            status: None,
            is_admin: false,
            is_present: None,
        },
        (None, Some((status, is_admin, is_present, _))) => UserPresentation {
            user_id,
            display_name: user_id.to_string(),
            is_bot: false,
            status: Some(status.clone()),
            is_admin: *is_admin,
            is_present: Some(*is_present),
        },
        (None, None) => UserPresentation {
            user_id,
            display_name: user_id.to_string(),
            is_bot: false,
            status: None,
            is_admin: false,
            is_present: None,
        },
    };

    let observed_at = member
        .as_ref()
        .and_then(|(_, _, _, observed_at)| observed_at.as_deref())
        .unwrap_or("нет данных");

    let (
        first_seen_at,
        last_seen_at,
        first_message_id,
        last_message_id,
        messages,
        replies,
        links,
        media,
        replies_to_channel_posts,
        replies_to_bot,
    ) = user_data
        .map(
            |(
                first_seen_at,
                last_seen_at,
                first_message_id,
                last_message_id,
                messages,
                replies,
                links,
                media,
                replies_to_channel_posts,
                replies_to_bot,
            )| {
                (
                    first_seen_at.unwrap_or_else(|| "нет данных".to_string()),
                    last_seen_at.unwrap_or_else(|| "нет данных".to_string()),
                    first_message_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "нет данных".to_string()),
                    last_message_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "нет данных".to_string()),
                    messages,
                    replies,
                    links,
                    media,
                    replies_to_channel_posts,
                    replies_to_bot,
                )
            },
        )
        .unwrap_or_else(|| {
            (
                "нет данных".to_string(),
                "нет данных".to_string(),
                "нет данных".to_string(),
                "нет данных".to_string(),
                totals.0,
                totals.1,
                totals.2,
                totals.3,
                totals.4,
                totals.5,
            )
        });

    Ok(format!(
        "<b>Статистика пользователя</b>\n{}\nСтатус обновлён: <code>{}</code>\nПервое сообщение: <code>{}</code> (#<code>{}</code>)\nПоследнее сообщение: <code>{}</code> (#<code>{}</code>)\n\nСообщения: <b>{}</b>\nРеплаи: <b>{}</b>\nРеплаи на посты: <b>{}</b>\nРеплаи на бота: <b>{}</b>\nСсылки: <b>{}</b>, медиа: <b>{}</b>\nАктивных дней: <b>{}</b>\nРеакций поставил: <b>{}</b>\nРеакций получил: <b>{}</b>",
        user.linked_with_badges(),
        escape_html(observed_at),
        escape_html(&first_seen_at),
        escape_html(&first_message_id),
        escape_html(&last_seen_at),
        escape_html(&last_message_id),
        messages,
        replies,
        replies_to_channel_posts,
        replies_to_bot,
        links,
        media,
        totals.6,
        reactions_given,
        reactions_received,
    ))
}

async fn resolve_user_id(pool: &PgPool, target: &str) -> anyhow::Result<Option<i64>> {
    let clean = target.trim();
    if clean.is_empty() {
        return Ok(None);
    }

    if let Ok(user_id) = clean.parse::<i64>() {
        return Ok(Some(user_id));
    }

    let username = clean.trim_start_matches('@').to_lowercase();
    let row = sqlx::query_as::<_, (i64,)>(
        r#"
        select telegram_user_id
        from telegram_user_profiles
        where lower(username) = $1
        order by updated_at desc
        limit 1
        "#,
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(user_id,)| user_id))
}
