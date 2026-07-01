use sqlx::PgPool;
use teloxide::prelude::*;
use teloxide::types::UserId;

use crate::config::Config;
use crate::db::telegram::upsert_user_profile;
use crate::features::stats::types::{
    ChatStatsSummary, StatsPeriod, UserPresentation, display_name,
};
use crate::telegram::html::{Html, truncate_text};
use crate::telegram::render::{escape_html, send_html};
use crate::text::normalize_ai_markers;

const TOP_LIMIT: i64 = 20;

#[derive(sqlx::FromRow)]
struct TopReactedRow {
    message_id: i32,
    user_id: i64,
    username: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    is_bot: bool,
    status: String,
    is_admin: bool,
    is_present: bool,
    text: Option<String>,
    has_photo: bool,
    has_video: bool,
    has_document: bool,
    has_audio: bool,
    has_voice: bool,
    has_sticker: bool,
    has_animation: bool,
    total_count: i32,
    reactions: serde_json::Value,
    created_at: String,
}

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

pub async fn send_top_messages(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
) -> ResponseResult<()> {
    let report = build_top_messages_report(pool, config)
        .await
        .map_err(|err| {
            tracing::error!(%err, "failed to build top messages report");
            teloxide::RequestError::Io(std::io::Error::other("top messages failed"))
        })?;

    send_html(bot, chat_id, report).await?;

    Ok(())
}

pub async fn send_top_reacted(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
) -> ResponseResult<()> {
    let report = build_top_reacted_report(pool, config)
        .await
        .map_err(|err| {
            tracing::error!(%err, "failed to build top reacted report");
            teloxide::RequestError::Io(std::io::Error::other("top reacted failed"))
        })?;

    send_html(bot, chat_id, report).await?;

    Ok(())
}

pub async fn send_user_stats(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    target: Option<&str>,
    reply_user_id: Option<i64>,
) -> ResponseResult<()> {
    if let Some(user_id) = numeric_target_user_id(target).or(reply_user_id) {
        refresh_user_profile_from_telegram(bot, pool, config, user_id).await;
    }

    let report = build_user_stats_report(pool, config, target, reply_user_id)
        .await
        .map_err(|err| {
            tracing::error!(%err, "failed to build user stats");
            teloxide::RequestError::Io(std::io::Error::other("user stats failed"))
        })?;

    send_html(bot, chat_id, report).await?;

    Ok(())
}

async fn build_top_messages_report(pool: &PgPool, config: &Config) -> anyhow::Result<String> {
    let rows = sqlx::query_as::<
        _,
        (
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
            bool,
            String,
            bool,
            bool,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
        ),
    >(
        r#"
        select m.user_id,
               p.username,
               p.first_name,
               p.last_name,
               coalesce(p.is_bot, false) as is_bot,
               coalesce(s.status, 'unknown') as status,
               coalesce(s.is_admin, false) as is_admin,
               coalesce(s.is_present, false) as is_present,
               count(*)::bigint as messages,
               count(*) filter (where m.reply_to_message_id is not null)::bigint as replies,
               count(*) filter (where m.has_photo or m.has_video or m.has_document or m.has_audio or m.has_voice or m.has_sticker or m.has_animation)::bigint as media,
               count(*) filter (where m.has_voice)::bigint as voices,
               count(*) filter (where m.has_links)::bigint as links,
               coalesce(sum(rc.total_count), 0)::bigint as reactions_received
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = m.chat_id and s.telegram_user_id = m.user_id
        left join telegram_message_reaction_counts rc on rc.chat_id = m.chat_id and rc.message_id = m.message_id
        where m.chat_id = $1
          and m.user_id is not null
          and m.source_channel_id is null
          and m.user_id <> 777000
          and coalesce(p.is_bot, false) = false
        group by m.user_id, p.username, p.first_name, p.last_name, p.is_bot, s.status, s.is_admin, s.is_present
        order by messages desc, reactions_received desc
        limit $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(TOP_LIMIT)
    .fetch_all(pool)
    .await?;

    let mut report = String::from("<b>Топ пишущих</b>\nЗа всё время\n");

    if rows.is_empty() {
        report.push_str("\nНет данных.");
        return Ok(report);
    }

    for (
        index,
        (
            user_id,
            username,
            first_name,
            last_name,
            is_bot,
            status,
            is_admin,
            is_present,
            messages,
            replies,
            media,
            voices,
            links,
            reactions_received,
        ),
    ) in rows.into_iter().enumerate()
    {
        let user = UserPresentation {
            user_id,
            display_name: display_name(
                username.as_deref(),
                first_name.as_deref(),
                last_name.as_deref(),
                user_id,
            ),
            is_bot,
            status: Some(status),
            is_admin,
            is_present: Some(is_present),
        };

        report.push_str(&format!(
            "\n{}. {}: <b>{}</b> соо, {} reply, {} медиа, {} голосовых, {} ссылок, {} реакций",
            index + 1,
            user.linked_with_badges(),
            messages,
            replies,
            media,
            voices,
            links,
            reactions_received
        ));
    }

    Ok(report)
}

async fn build_top_reacted_report(pool: &PgPool, config: &Config) -> anyhow::Result<String> {
    let rows = sqlx::query_as::<_, TopReactedRow>(
        r#"
        select m.message_id,
               m.user_id,
               p.username,
               p.first_name,
               p.last_name,
               coalesce(p.is_bot, false) as is_bot,
               coalesce(s.status, 'unknown') as status,
               coalesce(s.is_admin, false) as is_admin,
               coalesce(s.is_present, false) as is_present,
               m.text,
               m.has_photo,
               m.has_video,
               m.has_document,
               m.has_audio,
               m.has_voice,
               m.has_sticker,
               m.has_animation,
               rc.total_count,
               rc.reactions,
               to_char(m.created_at at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI') as created_at
        from telegram_message_reaction_counts rc
        join telegram_messages m on m.chat_id = rc.chat_id and m.message_id = rc.message_id
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = m.chat_id and s.telegram_user_id = m.user_id
        where rc.chat_id = $1
          and rc.total_count > 0
          and m.user_id is not null
          and m.source_channel_id is null
          and m.user_id <> 777000
          and coalesce(p.is_bot, false) = false
        order by rc.total_count desc, m.created_at desc
        limit $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(TOP_LIMIT)
    .fetch_all(pool)
    .await?;

    let mut report = String::from("<b>Топ сообщений по реакциям</b>\nЗа всё время\n");

    if rows.is_empty() {
        report.push_str("\nНет данных.");
        return Ok(report);
    }

    for (index, row) in rows.into_iter().enumerate() {
        let user = UserPresentation {
            user_id: row.user_id,
            display_name: display_name(
                row.username.as_deref(),
                row.first_name.as_deref(),
                row.last_name.as_deref(),
                row.user_id,
            ),
            is_bot: row.is_bot,
            status: Some(row.status),
            is_admin: row.is_admin,
            is_present: Some(row.is_present),
        };
        let preview = message_preview(
            row.text.as_deref(),
            row.has_photo,
            row.has_video,
            row.has_document,
            row.has_audio,
            row.has_voice,
            row.has_sticker,
            row.has_animation,
        );
        let message_link = Html::link(
            format!("#{}", row.message_id),
            message_url(config.discussion_chat_id, row.message_id),
        )
        .into_string();
        let reaction_summary = reaction_summary(&row.reactions, 4);

        report.push_str(&format!(
            "\n{}. {}: <b>{}</b> реакций{} от {}, <code>{}</code>\n{}",
            index + 1,
            message_link,
            row.total_count,
            reaction_summary,
            user.linked_with_badges(),
            escape_html(&row.created_at),
            Html::text(truncate_text(&preview, 80)).into_string()
        ));
    }

    Ok(report)
}

async fn refresh_user_profile_from_telegram(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    config: &Config,
    user_id: i64,
) {
    let Ok(user_id) = u64::try_from(user_id) else {
        return;
    };

    match bot
        .get_chat_member(ChatId(config.discussion_chat_id), UserId(user_id))
        .await
    {
        Ok(member) => {
            if let Err(err) = upsert_user_profile(pool, &member.user).await {
                tracing::warn!(%err, "failed to save refreshed user profile");
            }
        }
        Err(err) => {
            tracing::debug!(%err, user_id, "failed to refresh user profile from Telegram");
        }
    }
}

fn numeric_target_user_id(target: Option<&str>) -> Option<i64> {
    target?.trim().parse().ok()
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

fn message_preview(
    text: Option<&str>,
    has_photo: bool,
    has_video: bool,
    has_document: bool,
    has_audio: bool,
    has_voice: bool,
    has_sticker: bool,
    has_animation: bool,
) -> String {
    if let Some(text) = text.map(str::trim).filter(|value| !value.is_empty()) {
        return normalize_ai_markers(text);
    }

    let media = [
        (has_photo, "фото"),
        (has_video, "видео"),
        (has_document, "файл"),
        (has_audio, "аудио"),
        (has_voice, "голосовое"),
        (has_sticker, "стикер"),
        (has_animation, "GIF"),
    ]
    .into_iter()
    .filter_map(|(enabled, label)| enabled.then_some(label))
    .collect::<Vec<_>>();

    if media.is_empty() {
        "сообщение без текста".to_string()
    } else {
        format!("медиа: {}", media.join(", "))
    }
}

fn reaction_summary(reactions: &serde_json::Value, limit: usize) -> String {
    let Some(items) = reactions.as_array() else {
        return String::new();
    };

    let parts = items
        .iter()
        .filter_map(|item| {
            let count = item.get("count")?.as_i64()?;
            if count <= 0 {
                return None;
            }

            let label = item
                .get("emoji")
                .and_then(serde_json::Value::as_str)
                .or_else(|| item.get("type").and_then(serde_json::Value::as_str))
                .unwrap_or("reaction");

            Some(format!("{label} {count}"))
        })
        .take(limit)
        .collect::<Vec<_>>();

    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

async fn build_user_stats_report(
    pool: &PgPool,
    config: &Config,
    target: Option<&str>,
    reply_user_id: Option<i64>,
) -> anyhow::Result<String> {
    let Some(user_id) = resolve_user_id(pool, target, reply_user_id).await? else {
        let hint = match target.map(str::trim).filter(|value| !value.is_empty()) {
            Some(_) => "Не нашёл пользователя. Используй id, username из уже виденных ботом пользователей или reply на сообщение.".to_string(),
            None => "Не понял, кого смотреть. Отправь команду обычным сообщением, ответь ей на сообщение пользователя или передай id/username.".to_string(),
        };
        return Ok(hint);
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
            Option<i64>,
            Option<i64>,
            i64,
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
               floor(extract(epoch from (now() - first_seen_at)) / 86400)::bigint,
               floor(extract(epoch from (now() - last_seen_at)) / 86400)::bigint,
               message_count,
               reply_count,
               link_count,
               media_count,
               reply_to_channel_post_count,
               reply_to_bot_count,
               0::bigint as voice_count
        from telegram_chat_users
        where chat_id = $1 and telegram_user_id = $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let totals = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64, i64)>(
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
            where child.chat_id = $1
              and child.source_channel_id is null
        )
        select count(*)::bigint as messages,
               count(*) filter (where reply_to_message_id is not null)::bigint as replies,
               count(*) filter (where has_links)::bigint as links,
               count(*) filter (where has_photo or has_video or has_document or has_audio or has_voice or has_sticker or has_animation)::bigint as media,
               count(*) filter (where message_id in (select message_id from post_thread_messages))::bigint as post_comments,
               count(*) filter (where reply_to_message_id in (select bot_comment_message_id from post_comment_jobs))::bigint as replies_to_bot,
               count(distinct date_trunc('day', created_at at time zone 'Europe/Moscow' - interval '5 hours'))::bigint as active_days,
               count(*) filter (where has_voice)::bigint as voices
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
        first_seen_days_ago,
        last_seen_days_ago,
        messages,
        replies,
        links,
        media,
        replies_to_channel_posts,
        replies_to_bot,
        voices,
    ) = user_data
        .map(
            |(
                first_seen_at,
                last_seen_at,
                first_message_id,
                last_message_id,
                first_seen_days_ago,
                last_seen_days_ago,
                messages,
                replies,
                links,
                media,
                replies_to_channel_posts,
                replies_to_bot,
                voices,
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
                    first_seen_days_ago,
                    last_seen_days_ago,
                    messages,
                    replies,
                    links,
                    media,
                    replies_to_channel_posts,
                    replies_to_bot,
                    voices,
                )
            },
        )
        .unwrap_or_else(|| {
            (
                "нет данных".to_string(),
                "нет данных".to_string(),
                "нет данных".to_string(),
                "нет данных".to_string(),
                None,
                None,
                totals.0,
                totals.1,
                totals.2,
                totals.3,
                totals.4,
                totals.5,
                totals.7,
            )
        });

    let first_message = linked_message(
        config.discussion_chat_id,
        &first_seen_at,
        &first_message_id,
        first_seen_days_ago,
    );
    let last_message = linked_message(
        config.discussion_chat_id,
        &last_seen_at,
        &last_message_id,
        last_seen_days_ago,
    );

    Ok(format!(
        "<b>Статистика пользователя</b>\n{}\nСтатус обновлён: <code>{}</code>\nПервое сообщение: {}\nПоследнее сообщение: {}\n\nСообщения: <b>{}</b>\nРеплаи: <b>{}</b>\nКомментарии: <b>{}</b>\nРеплаи на бота: <b>{}</b>\nСсылки: <b>{}</b>, медиа: <b>{}</b>, голосовые: <b>{}</b>\nАктивных дней: <b>{}</b>\nРеакций поставил: <b>{}</b>\nРеакций получил: <b>{}</b>",
        user.linked_with_badges(),
        escape_html(observed_at),
        first_message,
        last_message,
        totals.0.max(messages),
        totals.1.max(replies),
        totals.4.max(replies_to_channel_posts),
        totals.5.max(replies_to_bot),
        totals.2.max(links),
        totals.3.max(media),
        totals.7.max(voices),
        totals.6,
        reactions_given,
        reactions_received,
    ))
}

async fn resolve_user_id(
    pool: &PgPool,
    target: Option<&str>,
    reply_user_id: Option<i64>,
) -> anyhow::Result<Option<i64>> {
    let clean = target.unwrap_or_default().trim();
    if clean.is_empty() {
        return Ok(reply_user_id);
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

fn linked_message(
    chat_id: i64,
    date_label: &str,
    message_id: &str,
    days_ago: Option<i64>,
) -> String {
    let label = match days_ago {
        Some(days) => format!("{date_label} ({days} дн. назад)"),
        None => date_label.to_string(),
    };

    match message_id.parse::<i32>() {
        Ok(message_id) => format!(
            "{} (#<code>{}</code>)",
            Html::link(label, message_url(chat_id, message_id)).into_string(),
            message_id
        ),
        Err(_) => format!(
            "{} (#<code>{}</code>)",
            escape_html(date_label),
            escape_html(message_id)
        ),
    }
}

fn message_url(chat_id: i64, message_id: i32) -> String {
    let internal_chat_id = chat_id
        .to_string()
        .strip_prefix("-100")
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| chat_id.abs().to_string());

    format!("https://t.me/c/{internal_chat_id}/{message_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaction_summary_formats_top_reactions() {
        let reactions = serde_json::json!([
            {"type": "emoji", "emoji": "🤣", "count": 67},
            {"type": "emoji", "emoji": "😢", "count": 8},
            {"type": "emoji", "emoji": "👍", "count": 1}
        ]);

        assert_eq!(reaction_summary(&reactions, 2), " (🤣 67, 😢 8)");
    }

    #[test]
    fn message_preview_falls_back_to_media() {
        assert_eq!(
            message_preview(None, true, false, false, false, true, false, false),
            "медиа: фото, голосовое"
        );
    }
}
