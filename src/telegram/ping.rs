//! Real Telegram reply first; bounded concurrent refresh then edits to the same message.
//! This deliberately uses RichEditInPlaceBackend even in private chats.
use std::time::{Duration, Instant};

use teloxide::{
    drafter::{DraftConfig, Drafter, EditAbortPolicy, RichEditInPlaceBackend},
    prelude::*,
    types::{InputRichMessage, ReplyParameters},
};

use crate::{
    features::health::{HealthRegistry, HealthSnapshot, HealthStatus},
    state::AppState,
    telegram::render::escape_html,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Public,
    Full,
    Probe,
}

fn parse_mode(args: &str, owner_dm: bool) -> Option<Mode> {
    match args.trim() {
        "" => Some(Mode::Public),
        "full" if owner_dm => Some(Mode::Full),
        "probe" if owner_dm => Some(Mode::Probe),
        _ => None,
    }
}

pub async fn handle(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    args: &str,
) -> ResponseResult<()> {
    let owner_dm = msg.chat.is_private()
        && msg.sender_chat.is_none()
        && msg.from.as_ref().is_some_and(|user| {
            !user.is_bot && state.config.owner_telegram_id == Some(user.id.0 as i64)
        });
    let Some(mode) = parse_mode(args, owner_dm) else {
        let text = if args.trim() == "full" || args.trim() == "probe" {
            "Расширенная диагностика доступна владельцу в личном чате."
        } else {
            "Использование: /ping, /ping full или /ping probe."
        };
        let send = async {
            bot.send_message(text)
                .chat_id(msg.chat.id)
                .reply_parameters(ReplyParameters::new(msg.id).allow_sending_without_reply())
                .await
        };
        let _ = tokio::time::timeout(Duration::from_secs(2), send).await;
        return Ok(());
    };

    let health = &state.health;
    let initial_snapshot = health.snapshot();
    let first = render(&initial_snapshot, state, mode, false, None);

    let mut backend = RichEditInPlaceBackend::new(bot.inner().clone(), msg.chat.id)
        .reply_parameters(ReplyParameters::new(msg.id).allow_sending_without_reply())
        .abort_policy(EditAbortPolicy::KeepPreview);
    if let Some(thread_id) = msg.thread_id {
        backend = backend.message_thread_id(thread_id);
    }
    let mut schedule = DraftConfig::default();
    schedule.coalesce_window = Duration::from_millis(1);
    schedule.min_update_interval = Duration::from_millis(650);
    schedule.request_timeout = Duration::from_secs(2);
    schedule.terminal_timeout = Duration::from_secs(2);
    schedule.retry_initial = Duration::from_millis(200);
    schedule.retry_max = Duration::from_millis(400);
    schedule.terminal_retry_budget = 0;

    let (drafter, sink) = match Drafter::snapshots(backend, state.drafter_limiter.clone(), schedule)
    {
        Ok(value) => value,
        Err(_) => {
            tracing::warn!("failed to initialize /ping edit-in-place delivery");
            return Ok(());
        }
    };
    if sink.update(rich(first.clone())).is_err() {
        tracing::warn!("failed to enqueue /ping initial real message");
        return Ok(());
    }
    let started_send = Instant::now();
    // flush must acknowledge the real send before any further work. No
    // message_id = no safe edit, and no speculative resend on unknown delivery.
    match tokio::time::timeout(Duration::from_secs(2), drafter.flush()).await {
        Ok(Ok(_)) => {}
        Ok(Err(_)) | Err(_) => {
            tracing::warn!("initial /ping delivery not confirmed; no duplicate send");
            return Ok(());
        }
    }
    let send_ms = crate::features::health::milliseconds(started_send.elapsed());

    // Persist the command only after acknowledging the real Telegram reply;
    // a broken database must never prevent /ping itself from answering.
    if crate::features::ingest::is_managed_chat(&state.config, msg.chat.id.0) {
        let recorded_message = (*msg).clone();
        let pool = state.pool.clone();
        let config = state.config.clone();
        tokio::spawn(async move {
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                crate::features::ingest::ingest_message(&pool, &recorded_message, &config),
            )
            .await;
            if let Ok(Err(_)) | Err(_) = result {
                tracing::warn!("best-effort /ping ingestion failed or timed out");
            }
        });
    }
    let diagnostics_started = Instant::now();

    // The initial message was delivered before a single probe started.
    let mut changes = health.subscribe();
    health.refresh(state.pool.clone(), state.config.clone());
    #[cfg(feature = "ask")]
    if mode == Mode::Probe && state.config.ask_enabled {
        health.probe_agent_mcp(state.config.clone());
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut last_rendered = first;
    loop {
        let snapshot = health.snapshot();
        let next = render(&snapshot, state, mode, false, Some(send_ms));
        if next != last_rendered {
            if sink.update(rich(next.clone())).is_err() {
                tracing::warn!("failed to enqueue /ping edit");
            }
            last_rendered = next;
        }
        if !snapshot.busy {
            break;
        }
        tokio::select! {
            result = changes.changed() => {
                if result.is_err() {
                    break;
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    let final_snapshot = health.snapshot();
    let mut final_text = render(&final_snapshot, state, mode, true, Some(send_ms));
    final_text.push_str(&format!(
        "\nДиагностика: {} мс",
        crate::features::health::milliseconds(diagnostics_started.elapsed())
    ));
    match tokio::time::timeout(Duration::from_secs(2), drafter.finish(rich(final_text))).await {
        Ok(Ok(_)) => {}
        Ok(Err(_)) | Err(_) => {
            // Existing real message remains visible (KeepPreview). Sending
            // another one after an ambiguous edit could duplicate the reply.
            tracing::warn!("final /ping edit not confirmed; original message preserved");
        }
    }
    Ok(())
}

fn rich(html: String) -> InputRichMessage {
    InputRichMessage::html(html).skip_entity_detection(true)
}

fn sample_line(
    name: &str,
    id: &'static str,
    snapshot: &HealthSnapshot,
    full: bool,
    final_frame: bool,
) -> String {
    let Some(sample) = snapshot.checks.get(id) else {
        return format!(
            "{name}: {}",
            if final_frame { "UNKNOWN" } else { "CHECKING" }
        );
    };
    let mut status = sample.status;
    if final_frame && status == HealthStatus::Checking {
        status = HealthStatus::TimedOut;
    }
    if status == HealthStatus::Healthy && sample.observed_at.elapsed() > Duration::from_secs(120) {
        status = HealthStatus::Unknown;
    }
    let latency = sample
        .elapsed_ms
        .map_or_else(String::new, |ms| format!(" · {ms} мс"));
    let detail = if full && !sample.detail.is_empty() {
        // Existing error details are fixed, sanitized strings. Escape anyway.
        let safe: String = sample.detail.chars().take(240).collect();
        format!(" ({})", escape_html(&safe))
    } else {
        String::new()
    };
    format!("{name}: {}{latency}{detail}", status.text())
}

fn render(
    snapshot: &HealthSnapshot,
    state: &AppState,
    mode: Mode,
    final_frame: bool,
    send_ms: Option<u64>,
) -> String {
    let full = mode != Mode::Public;
    let mut lines = Vec::with_capacity(48);
    lines.push("<b>NedoBot · /ping</b>".to_owned());
    let secs = snapshot.uptime.as_secs();
    lines.push(format!(
        "Uptime: {}ч {:02}м {:02}с",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    ));
    if let Some(ms) = send_ms {
        lines.push(format!("Первый ответ: {ms} мс"));
    }

    lines.push(sample_line(
        "PostgreSQL",
        "postgres",
        snapshot,
        full,
        final_frame,
    ));
    lines.push(sample_line(
        "Очереди",
        "queues",
        snapshot,
        full,
        final_frame,
    ));

    let total = snapshot.workers.len();
    let stopped = snapshot.workers.values().filter(|v| v.stopped).count();
    let failing = snapshot
        .workers
        .values()
        .filter(|v| v.consecutive_failures >= 3)
        .count();
    lines.push(format!(
        "Workers: {} active / {} registered · errors={failing}",
        total.saturating_sub(stopped),
        total
    ));
    let busy_slots = state
        .config
        .ask_max_concurrency
        .saturating_sub(state.ask_slots.available_permits());
    lines.push(format!(
        "Agent /ask: {busy_slots}/{} active",
        state.config.ask_max_concurrency
    ));

    if full {
        for (name, id) in [
            ("Ask audit", "ask"),
            ("Voice", "voice"),
            ("Moderation", "moderation"),
            ("QR/URL", "qr"),
        ] {
            lines.push(sample_line(name, id, snapshot, true, final_frame));
        }
        for (name, sample) in &snapshot.workers {
            let state = if sample.stopped {
                "STOPPED"
            } else if sample.consecutive_failures >= 3 {
                "DEGRADED"
            } else if sample.last_poll_at.is_none() {
                "RUNNING"
            } else {
                "OK"
            };
            let ms = sample
                .last_poll_ms
                .map_or_else(|| "—".to_owned(), |x| format!("{x} мс"));
            lines.push(format!(
                "{}: {} · poll={} · failures={}",
                escape_html(name),
                state,
                ms,
                sample.failures
            ));
        }
    }
    if state.config.ask_enabled {
        lines.push(sample_line(
            "Agent MCP",
            "agent_mcp",
            snapshot,
            full,
            final_frame,
        ));
    } else {
        lines.push("Agent MCP: DISABLED".to_owned());
    }
    if full {
        lines.push(sample_line("LLM", "llm", snapshot, true, final_frame));
        lines.push(sample_line(
            "Search MCP",
            "search_mcp",
            snapshot,
            true,
            final_frame,
        ));
    }
    lines.push("Public MCP HTTP: UNKNOWN (отдельный процесс)".to_owned());

    let has_error = snapshot
        .workers
        .values()
        .any(|w| w.stopped || w.consecutive_failures >= 3)
        || snapshot.checks.iter().any(|(id, s)| {
            matches!(*id, "postgres" | "queues" | "moderation")
                && matches!(s.status, HealthStatus::Unavailable | HealthStatus::TimedOut)
        });
    let has_warning = snapshot.checks.values().any(|s| {
        matches!(
            s.status,
            HealthStatus::Degraded | HealthStatus::Unavailable | HealthStatus::TimedOut
        )
    });
    let missing_core = ["postgres", "queues"]
        .iter()
        .any(|id| !snapshot.checks.contains_key(id));
    let stale_core = ["postgres", "queues"].iter().any(|id| {
        snapshot.checks.get(id).is_some_and(|sample| {
            sample.status == HealthStatus::Unknown
                || (sample.status == HealthStatus::Healthy
                    && sample.observed_at.elapsed() > Duration::from_secs(120))
        })
    });
    let timed_out_core = final_frame
        && ["postgres", "queues"].iter().any(|id| {
            snapshot
                .checks
                .get(id)
                .is_some_and(|sample| sample.status == HealthStatus::Checking)
        });
    let overall = if !final_frame && (snapshot.busy || missing_core) {
        "CHECKING"
    } else if has_error || timed_out_core {
        "UNAVAILABLE"
    } else if has_warning || (final_frame && snapshot.busy) {
        "DEGRADED"
    } else if missing_core || stale_core {
        "UNKNOWN"
    } else {
        "OK / наблюдаемые компоненты"
    };
    lines.push(format!("<b>Итог: {overall}</b>"));
    if final_frame {
        lines.push("Проверка завершена".to_owned());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privileged_modes_are_restricted_to_owner_dm() {
        assert_eq!(parse_mode("full", false), None);
        assert_eq!(parse_mode("probe", false), None);
        assert_eq!(parse_mode("probe", true), Some(Mode::Probe));
        assert_eq!(parse_mode("", false), Some(Mode::Public));
    }

    #[test]
    fn unknown_is_never_reported_as_healthy() {
        let health = HealthRegistry::new();
        let snapshot = health.snapshot();
        assert!(sample_line("DB", "postgres", &snapshot, true, true).contains("UNKNOWN"));
    }
}
