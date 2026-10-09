use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use sqlx::PgPool;
use teloxide::prelude::*;
use teloxide::utils::time::TimeContext;
use tracing::Instrument;

use crate::config::Config;
use crate::features::stats::render_html;
use crate::features::stats::render_rich;
use crate::features::stats::service::{self, HTML_TOP_LIMIT, RICH_TOP_LIMIT};
use crate::features::stats::strings::StatsStrings;
use crate::features::stats::types::{StatsPeriod, StatsRender};
use crate::telegram::render::{send_html, send_rich_html};
use crate::telegram::service_messages::{self, MessageAudience};

static NEXT_USER_STATS_TRACE_ID: AtomicU64 = AtomicU64::new(1);
const TOP_WORD_LIMIT: i64 = 20;

/// Transport wiring for stats commands. Data is assembled in `service`; output is
/// formatted in the selected renderer. Neither renderer has database access.
#[allow(clippy::too_many_arguments)]
pub async fn send_chat_stats(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    stats_scope_chat_id: i64,
    render_time: &TimeContext,
    period: StatsPeriod,
    render: StatsRender,
    strings: &StatsStrings,
    audience: Option<MessageAudience>,
) -> ResponseResult<()> {
    let Some(audience) = audience else {
        return Ok(());
    };
    let mut data = service::chat_stats_report_data(pool, stats_scope_chat_id, period)
        .await
        .map_err(stats_error("failed to build chat stats"))?;
    data.member_count = chat_member_count(bot, stats_scope_chat_id).await;
    let report = match render {
        StatsRender::Html => render_html::chat_stats(&data, render_time, strings),
        StatsRender::Rich => {
            render_rich::chat_stats(&data, stats_scope_chat_id, render_time, strings)
        }
    };
    send_stats_report(bot, chat_id, report, render, audience).await?;
    Ok(())
}

pub async fn send_top_messages(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    stats_scope_chat_id: i64,
    render: StatsRender,
    strings: &StatsStrings,
    audience: Option<MessageAudience>,
) -> ResponseResult<()> {
    let Some(audience) = audience else {
        return Ok(());
    };
    service::refresh_top_message_users(bot, pool, stats_scope_chat_id).await;
    let limit = match render {
        StatsRender::Html => HTML_TOP_LIMIT,
        StatsRender::Rich => RICH_TOP_LIMIT,
    };
    let data = service::top_messages_report_data(pool, stats_scope_chat_id, limit)
        .await
        .map_err(stats_error("failed to build top messages report"))?;
    let report = match render {
        StatsRender::Html => render_html::top_messages(&data, strings),
        StatsRender::Rich => render_rich::top_messages(&data, strings),
    };
    send_stats_report(bot, chat_id, report, render, audience).await?;
    Ok(())
}

pub async fn send_bottom_messages(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    stats_scope_chat_id: i64,
    render: StatsRender,
    strings: &StatsStrings,
    audience: Option<MessageAudience>,
) -> ResponseResult<()> {
    let Some(audience) = audience else {
        return Ok(());
    };
    service::refresh_top_message_users(bot, pool, stats_scope_chat_id).await;
    let limit = match render {
        StatsRender::Html => HTML_TOP_LIMIT,
        StatsRender::Rich => RICH_TOP_LIMIT,
    };
    let data = service::bottom_messages_report_data(pool, stats_scope_chat_id, limit)
        .await
        .map_err(stats_error("failed to build bottom messages report"))?;
    let report = match render {
        StatsRender::Html => render_html::bottom_messages(&data, strings),
        StatsRender::Rich => render_rich::bottom_messages(&data, strings),
    };
    send_stats_report(bot, chat_id, report, render, audience).await?;
    Ok(())
}

pub async fn send_top_reacted(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    stats_scope_chat_id: i64,
    render: StatsRender,
    strings: &StatsStrings,
    audience: Option<MessageAudience>,
) -> ResponseResult<()> {
    let Some(audience) = audience else {
        return Ok(());
    };
    service::refresh_top_reacted_users(bot, pool, stats_scope_chat_id).await;
    let limit = match render {
        StatsRender::Html => HTML_TOP_LIMIT,
        StatsRender::Rich => RICH_TOP_LIMIT,
    };
    let data = service::top_reacted_report_data(pool, stats_scope_chat_id, limit)
        .await
        .map_err(stats_error("failed to build top reacted report"))?;
    let report = match render {
        StatsRender::Html => render_html::top_reacted(&data, stats_scope_chat_id, strings),
        StatsRender::Rich => render_rich::top_reacted(&data, stats_scope_chat_id, strings),
    };
    send_stats_report(bot, chat_id, report, render, audience).await?;
    Ok(())
}

// This matches the neighboring stats transport functions; `word` is its
// report-specific input, so a shared request struct would add no domain value.
#[allow(clippy::too_many_arguments)]
pub async fn send_top_word(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    stats_scope_chat_id: i64,
    word: &str,
    render: StatsRender,
    strings: &StatsStrings,
    audience: Option<MessageAudience>,
) -> ResponseResult<()> {
    let Some(audience) = audience else {
        return Ok(());
    };
    let data = service::top_word_report_data(pool, stats_scope_chat_id, word, TOP_WORD_LIMIT)
        .await
        .map_err(stats_error("failed to build top word report"))?;
    let report = match render {
        StatsRender::Html => render_html::top_word(&data, strings),
        StatsRender::Rich => render_rich::top_word(&data, strings),
    };
    send_stats_report(bot, chat_id, report, render, audience).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn send_user_stats(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    stats_scope_chat_id: i64,
    target: Option<&str>,
    reply_user_id: Option<i64>,
    render: StatsRender,
    strings: &StatsStrings,
    audience: Option<MessageAudience>,
) -> ResponseResult<()> {
    let Some(audience) = audience else {
        return Ok(());
    };
    let trace_id = NEXT_USER_STATS_TRACE_ID.fetch_add(1, Ordering::Relaxed);
    let render_name = stats_render_name(render);
    let target_kind = user_stats_target_kind(target, reply_user_id);
    let span = tracing::info_span!("userstatus", trace_id, render = render_name, target_kind);
    let mut timer = UserStatsCommandTimer::new(trace_id);
    let result = async {
        let profile_refresh_started = Instant::now();
        let refresh_user_id = numeric_target_user_id(target).or(reply_user_id);
        if let Some(user_id) = refresh_user_id {
            service::refresh_user_profile(bot, pool, stats_scope_chat_id, user_id, trace_id).await;
        }
        log_user_stats_stage(
            trace_id,
            "profile_refresh",
            profile_refresh_started,
            if refresh_user_id.is_some() {
                "best_effort"
            } else {
                "skipped"
            },
        );

        let report_data_started = Instant::now();
        let report_data_result = service::user_stats_report_data(
            pool,
            stats_scope_chat_id,
            target,
            reply_user_id,
            trace_id,
        )
        .await;
        log_user_stats_stage(
            trace_id,
            "report_data",
            report_data_started,
            if report_data_result.is_ok() {
                "ok"
            } else {
                "error"
            },
        );
        let mut data = report_data_result.map_err(stats_error("failed to build user stats"))?;

        let avatar_started = Instant::now();
        let avatar_skipped = render != StatsRender::Rich || data.is_none();
        if let (StatsRender::Rich, Some(data)) = (render, data.as_mut()) {
            service::enrich_user_stats_avatar(bot, config, data).await;
        }
        log_user_stats_stage(
            trace_id,
            "avatar_enrichment",
            avatar_started,
            if avatar_skipped {
                "skipped"
            } else {
                "best_effort"
            },
        );

        let render_started = Instant::now();
        let report = match render {
            StatsRender::Html => {
                render_html::user_stats(data.as_ref(), target, stats_scope_chat_id, strings)
            }
            StatsRender::Rich => {
                render_rich::user_stats(data.as_ref(), target, stats_scope_chat_id, strings)
            }
        };
        log_user_stats_stage(trace_id, "render", render_started, "ok");

        let delivery_started = Instant::now();
        let delivery_result = send_stats_report(bot, chat_id, report, render, audience).await;
        log_user_stats_stage(
            trace_id,
            "telegram_delivery",
            delivery_started,
            if delivery_result.is_ok() {
                "ok"
            } else {
                "error"
            },
        );
        delivery_result?;
        Ok(())
    }
    .instrument(span)
    .await;
    timer.set_outcome(if result.is_ok() { "ok" } else { "error" });
    result
}

fn stats_render_name(render: StatsRender) -> &'static str {
    match render {
        StatsRender::Html => "html",
        StatsRender::Rich => "rich",
    }
}

fn user_stats_target_kind(target: Option<&str>, reply_user_id: Option<i64>) -> &'static str {
    if numeric_target_user_id(target).is_some() {
        "id"
    } else if target.is_some() {
        "username"
    } else if reply_user_id.is_some() {
        "reply"
    } else {
        "none"
    }
}

fn log_user_stats_stage(
    trace_id: u64,
    stage: &'static str,
    started: Instant,
    outcome: &'static str,
) {
    tracing::info!(
        target: "nedobot::stats::perf",
        trace_id,
        stage,
        elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0,
        outcome,
        "userstatus timing"
    );
}

struct UserStatsCommandTimer {
    trace_id: u64,
    started: Instant,
    outcome: &'static str,
}

impl UserStatsCommandTimer {
    fn new(trace_id: u64) -> Self {
        Self {
            trace_id,
            started: Instant::now(),
            outcome: "cancelled",
        }
    }

    fn set_outcome(&mut self, outcome: &'static str) {
        self.outcome = outcome;
    }
}

impl Drop for UserStatsCommandTimer {
    fn drop(&mut self) {
        tracing::info!(
            target: "nedobot::stats::perf",
            trace_id = self.trace_id,
            stage = "total",
            elapsed_ms = self.started.elapsed().as_secs_f64() * 1_000.0,
            outcome = self.outcome,
            "userstatus timing"
        );
    }
}

async fn send_stats_report(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    report: String,
    render: StatsRender,
    audience: MessageAudience,
) -> ResponseResult<Message> {
    match (audience, render) {
        (MessageAudience::Public, StatsRender::Html) => send_html(bot, chat_id, report).await,
        (MessageAudience::Public, StatsRender::Rich) => send_rich_html(bot, chat_id, report).await,
        (audience, StatsRender::Html) => {
            service_messages::send_html(bot, chat_id, report, audience).await
        }
        (audience, StatsRender::Rich) => {
            service_messages::send_rich_html(bot, chat_id, report, audience).await
        }
    }
}

fn stats_error(message: &'static str) -> impl FnOnce(anyhow::Error) -> teloxide::RequestError {
    move |err| {
        tracing::error!(%err, "{message}");
        teloxide::RequestError::Io(std::io::Error::other("stats failed").into())
    }
}

fn numeric_target_user_id(target: Option<&str>) -> Option<i64> {
    target?.parse().ok()
}

/// Best-effort live member count for the engagement denominator. Failures
/// stay silent in the report (a dash) and loud in logs.
async fn chat_member_count(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: i64,
) -> Option<i64> {
    use teloxide::prelude::Requester;
    match bot
        .get_chat_member_count(teloxide::types::ChatId(chat_id))
        .await
    {
        Ok(count) => Some(count as i64),
        Err(err) => {
            tracing::warn!(%err, chat_id, "failed to get chat member count");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::features::stats::render_html::message_preview;
    use crate::features::stats::strings::StatsStrings;
    use crate::features::stats::types::MessageMediaPreview;

    #[test]
    fn message_preview_falls_back_to_media() {
        assert_eq!(
            message_preview(
                None,
                MessageMediaPreview {
                    has_voice: true,
                    ..Default::default()
                },
                &StatsStrings::russian(),
            ),
            "медиа: голосовое"
        );
    }
}
