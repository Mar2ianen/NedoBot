//! Adapter ручного ban/unban канала; права проверяет вызывающий manual handler.
use crate::features::manual_moderation::{
    sender_chat::{
        self, Action, PENDING_PAGE_SIZE, PendingPage, ReconcileOutcome, Request, Reservation,
        Resolution,
    },
    types::{CommandKind, ParsedCommand},
};
use crate::state::AppState;
use teloxide::{adaptors::DefaultParseMode, prelude::*, types::Chat};

pub(super) fn target(
    msg: &Message,
    parsed: &ParsedCommand,
) -> anyhow::Result<Option<(Action, Chat)>> {
    let Some(reply) = msg.reply_to_message() else {
        return Ok(None);
    };
    let Some(sender) = reply.sender_chat.as_ref() else {
        return Ok(None);
    };
    anyhow::ensure!(
        reply.chat.id == msg.chat.id,
        "reply должен быть из этого же чата"
    );
    anyhow::ensure!(
        sender.is_channel() && sender.id != msg.chat.id && !reply.is_automatic_forward(),
        "Бан доступен только для комментария от канала; исходный пост и анонимный администратор не являются такой целью."
    );
    anyhow::ensure!(
        parsed.targets.is_empty() && parsed.duration.is_none(),
        "Для канала используй /ban или /unban по reply без ID и срока; причину можно указать через --."
    );
    let action = match parsed.kind {
        CommandKind::Ban => Action::Ban,
        CommandKind::Unban => Action::Unban,
        _ => anyhow::bail!("Для канала поддерживаются только /ban и /unban по reply."),
    };
    Ok(Some((action, sender.clone())))
}

pub(super) async fn execute(
    bot: &DefaultParseMode<Bot>,
    pool: &sqlx::PgPool,
    msg: &Message,
    actor: UserId,
    action: Action,
    sender: &Chat,
    reason: Option<&str>,
) -> anyhow::Result<String> {
    let reservation = sender_chat::reserve(
        pool,
        &Request {
            chat_id: msg.chat.id.0,
            command_message_id: msg.id.0,
            sender_chat_id: sender.id.0,
            actor_user_id: actor.0 as i64,
            action,
            reason,
        },
    )
    .await?;
    let id = match reservation {
        Reservation::Succeeded => return Ok(success(action)),
        Reservation::Failed => {
            return Ok(
                "Эта команда канала уже подтверждена как невыполненная. Отправь новую команду."
                    .into(),
            );
        }
        Reservation::Uncertain(id) | Reservation::Busy(id) => {
            return Ok(format!(
                "Для этого канала есть операция #{id} с неподтверждённым исходом. Повтор не выполнен; проверь ограничения в Telegram и подтверди состояние: /channelops {id} applied или /channelops {id} not_applied."
            ));
        }
        Reservation::Started(id) => id,
    };
    let call = async {
        match action {
            Action::Ban => bot.ban_chat_sender_chat(msg.chat.id, sender.id).await,
            Action::Unban => bot.unban_chat_sender_chat(msg.chat.id, sender.id).await,
        }
    };
    // Типизированный ответ teloxide — `True`: `false` из Bot API не может
    // десериализоваться как успех и попадёт в unknown, а не в succeeded.
    let acknowledged = matches!(
        tokio::time::timeout(std::time::Duration::from_secs(15), call).await,
        Ok(Ok(_))
    );
    sender_chat::finish(pool, id, acknowledged).await?;
    if acknowledged {
        Ok(success(action))
    } else {
        Ok("Telegram не подтвердил результат операции с каналом. Проверь ограничения вручную; автоматического повтора не будет. После проверки подтверди состояние через /channelops.".into())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReconcileRequest {
    List {
        page: u32,
    },
    Resolve {
        operation_id: i64,
        resolution: Resolution,
        note: Option<String>,
    },
}

fn parse_reconcile_request(args: &str) -> anyhow::Result<ReconcileRequest> {
    let (head, note) = match args.split_once("--") {
        Some((head, note)) => {
            anyhow::ensure!(
                !note.contains("--"),
                "используй разделитель -- только один раз"
            );
            let note = note.trim();
            (head.trim(), (!note.is_empty()).then(|| note.to_string()))
        }
        None => (args.trim(), None),
    };
    let tokens = head.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        anyhow::ensure!(
            note.is_none(),
            "примечание можно указать только вместе с подтверждением"
        );
        return Ok(ReconcileRequest::List { page: 1 });
    }
    if tokens.len() == 1 {
        anyhow::ensure!(
            note.is_none(),
            "примечание можно указать только вместе с подтверждением"
        );
        let page = tokens[0]
            .parse::<u32>()
            .ok()
            .filter(|page| (1..=10_000).contains(page))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "страница должна быть числом от 1 до 10000; для подтверждения используй /channelops <id> applied|not_applied"
                )
            })?;
        return Ok(ReconcileRequest::List { page });
    }
    anyhow::ensure!(
        tokens.len() == 2,
        "используй /channelops [страница] или /channelops <id> applied|not_applied [-- примечание]"
    );
    let operation_id = tokens[0]
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| anyhow::anyhow!("id операции должен быть положительным числом"))?;
    let resolution = Resolution::parse(tokens[1])?;
    anyhow::ensure!(
        note.as_ref().is_none_or(|note| note.chars().count() <= 500),
        "примечание длиннее 500 символов"
    );
    Ok(ReconcileRequest::Resolve {
        operation_id,
        resolution,
        note,
    })
}

/// Административное подтверждение состояния канала. Команда не вызывает
/// Telegram: администратор сначала проверяет ограничение вручную.
pub(super) async fn handle_reconcile(
    bot: &DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    args: &str,
) -> ResponseResult<()> {
    let Some(actor) = msg.from.as_ref().filter(|user| !user.is_bot) else {
        super::manual_moderation::send_reply(
            bot,
            msg,
            state,
            "Команда должна быть отправлена администратором от личного аккаунта.",
        )
        .await?;
        return Ok(());
    };
    let Some(actor) =
        super::manual_moderation::authorize_actor(bot, msg.chat.id, actor.id, true).await
    else {
        super::manual_moderation::send_reply(
            bot,
            msg,
            state,
            "Недостаточно прав: проверь права администратора и разрешение бота ограничивать участников.",
        )
        .await?;
        return Ok(());
    };
    let request = match parse_reconcile_request(args) {
        Ok(request) => request,
        Err(error) => {
            super::manual_moderation::send_reply(bot, msg, state, &error.to_string()).await?;
            return Ok(());
        }
    };
    let text = match request {
        ReconcileRequest::List { page } => {
            match sender_chat::pending(&state.pool, msg.chat.id.0, page).await {
                Ok(page) => render_pending(&page),
                Err(error) => {
                    tracing::error!(%error, "failed to list manual channel operations");
                    "Не удалось прочитать журнал операций с каналами.".to_string()
                }
            }
        }
        ReconcileRequest::Resolve {
            operation_id,
            resolution,
            note,
        } => match sender_chat::reconcile(
            &state.pool,
            msg.chat.id.0,
            operation_id,
            actor.user_id.0 as i64,
            resolution,
            note.as_deref(),
        )
        .await
        {
            Ok(outcome) => render_reconcile(operation_id, resolution, outcome),
            Err(error) => {
                tracing::error!(%error, operation_id, "failed to reconcile manual channel operation");
                format!("Не удалось сохранить подтверждение операции #{operation_id}.")
            }
        },
    };
    super::manual_moderation::send_reply(bot, msg, state, &text).await?;
    Ok(())
}

const MAX_REASON_PREVIEW_CHARS: usize = 80;

fn render_pending(page: &PendingPage) -> String {
    if page.total == 0 {
        return "Незавершённых операций с каналами нет.".to_string();
    }
    let total_pages = u32::try_from((page.total - 1) / PENDING_PAGE_SIZE + 1).unwrap_or(u32::MAX);
    if page.operations.is_empty() {
        return format!(
            "На странице {} незавершённых операций нет. Всего: {}; доступны страницы 1..={total_pages}.",
            page.page, page.total
        );
    }
    let mut lines = vec![format!(
        "Незавершённые операции с каналами (страница {} из {total_pages}, всего {}):",
        page.page, page.total
    )];
    for operation in &page.operations {
        let reason = operation
            .reason
            .as_deref()
            .map(reason_preview)
            .filter(|reason| !reason.is_empty())
            .map(|reason| format!(" · причина: {reason}"))
            .unwrap_or_default();
        lines.push(format!(
            "#{} {} канала {} · {} · создана {} · admin {} · команда {}{}",
            operation.id,
            action_label(&operation.action),
            operation.sender_chat_id,
            status_label(&operation.status),
            operation.created_at.format("%d.%m.%Y %H:%M UTC"),
            operation.actor_user_id,
            operation.command_message_id,
            reason
        ));
    }
    if total_pages > 1 {
        let mut navigation = Vec::new();
        if page.page > 1 {
            navigation.push(format!("/channelops {} — предыдущая", page.page - 1));
        }
        if page.page < total_pages {
            navigation.push(format!("/channelops {} — следующая", page.page + 1));
        }
        if !navigation.is_empty() {
            lines.push(format!("Навигация: {}.", navigation.join(", ")));
        }
    }
    lines.push(
        "Проверь ограничение в Telegram и подтверди: /channelops <id> applied|not_applied [-- примечание]."
            .to_string(),
    );
    lines.join("\n")
}

/// Preview причины не должен раздувать сообщение HTML-экранированием.
fn reason_preview(reason: &str) -> String {
    let flattened = reason
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | '&' | '"') {
                ' '
            } else {
                c
            }
        })
        .collect::<String>();
    let flattened = flattened.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = flattened.chars();
    let preview: String = chars.by_ref().take(MAX_REASON_PREVIEW_CHARS).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

fn render_reconcile(
    operation_id: i64,
    resolution: Resolution,
    outcome: ReconcileOutcome,
) -> String {
    match outcome {
        ReconcileOutcome::Resolved {
            previous_status,
            sender_chat_id,
            action,
        } => {
            let confirmed = match resolution {
                Resolution::Applied => "выполнена",
                Resolution::NotApplied => "не выполнена",
            };
            format!(
                "Операция #{operation_id} ({}) канала {sender_chat_id} подтверждена как {confirmed} (было: {previous_status}). Telegram не вызывался; для новой попытки отправь новую команду.",
                action_label(&action)
            )
        }
        ReconcileOutcome::NotFound => {
            format!("Операция #{operation_id} не найдена в этом чате.")
        }
        ReconcileOutcome::AlreadyResolved { status } => format!(
            "Операция #{operation_id} уже имеет подтверждённое состояние {status}; повторное подтверждение не требуется."
        ),
        ReconcileOutcome::TooRecent {
            retry_after_seconds,
        } => format!(
            "Операция #{operation_id} ещё может выполняться. Проверь Telegram и повтори подтверждение не раньше чем через {retry_after_seconds} сек."
        ),
    }
}

fn status_label(status: &str) -> &'static str {
    match status {
        "started" => "запущена, исход не записан",
        "unknown" => "исход не подтверждён",
        _ => "неизвестное состояние",
    }
}

fn action_label(action: &str) -> &'static str {
    match action {
        "ban" => "бан",
        "unban" => "разбан",
        _ => "действие",
    }
}

fn success(action: Action) -> String {
    match action {
        Action::Ban => "Отправка сообщений от этого канала в чате заблокирована.",
        Action::Unban => "Блокировка отправки сообщений от этого канала в чате снята.",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::manual_moderation::sender_chat::PendingOperation;
    use chrono::{TimeZone, Utc};

    #[test]
    fn reconcile_command_lists_the_first_page_without_arguments() {
        assert_eq!(
            parse_reconcile_request("").unwrap(),
            ReconcileRequest::List { page: 1 }
        );
        assert_eq!(
            parse_reconcile_request("   ").unwrap(),
            ReconcileRequest::List { page: 1 }
        );
        assert_eq!(
            parse_reconcile_request("2").unwrap(),
            ReconcileRequest::List { page: 2 }
        );
        assert_eq!(
            parse_reconcile_request("10000").unwrap(),
            ReconcileRequest::List { page: 10_000 }
        );
    }

    #[test]
    fn reconcile_command_requires_an_explicit_state() {
        assert_eq!(
            parse_reconcile_request("42 applied -- проверил Telegram").unwrap(),
            ReconcileRequest::Resolve {
                operation_id: 42,
                resolution: Resolution::Applied,
                note: Some("проверил Telegram".to_string()),
            }
        );
        assert_eq!(
            parse_reconcile_request("42 not_applied").unwrap(),
            ReconcileRequest::Resolve {
                operation_id: 42,
                resolution: Resolution::NotApplied,
                note: None,
            }
        );
        for args in [
            "42 maybe",
            "0 applied",
            "-1 applied",
            "42 applied extra",
            "42 applied -- -- повтор",
        ] {
            assert!(parse_reconcile_request(args).is_err(), "{args}");
        }
    }

    #[test]
    fn reconcile_command_bounds_pages_and_the_operator_note() {
        for args in ["0", "10001", "-1", "page", "2 -- примечание"] {
            assert!(parse_reconcile_request(args).is_err(), "{args}");
        }
        let note = "x".repeat(501);
        assert!(parse_reconcile_request(&format!("42 applied -- {note}")).is_err());
        let note = "x".repeat(500);
        assert!(parse_reconcile_request(&format!("42 applied -- {note}")).is_ok());
    }

    fn operation(id: i64, reason: Option<String>) -> PendingOperation {
        PendingOperation {
            id,
            sender_chat_id: i64::MIN,
            command_message_id: i32::MAX,
            actor_user_id: i64::MAX,
            action: "ban".to_string(),
            reason,
            status: "started".to_string(),
            created_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        }
    }

    #[test]
    fn pending_listing_is_bounded_and_paginated() {
        let operations = (0..PENDING_PAGE_SIZE)
            .map(|index| operation(i64::MAX - index, Some("<&>\"😀 ".repeat(100))))
            .collect();
        let text = render_pending(&PendingPage {
            operations,
            total: PENDING_PAGE_SIZE * 2,
            page: 1,
        });
        assert!(text.contains(&format!("#{}", i64::MAX)));
        assert!(text.contains("страница 1 из 2"));
        assert!(text.contains("/channelops 2 — следующая"));
        assert!(text.contains('…'));
        let escaped = crate::telegram::html::Html::text(&text).into_string();
        assert!(
            escaped.encode_utf16().count() <= 4096,
            "{}",
            escaped.encode_utf16().count()
        );

        let text = render_pending(&PendingPage {
            operations: vec![operation(1, None)],
            total: PENDING_PAGE_SIZE + 1,
            page: 2,
        });
        assert!(text.contains("страница 2 из 2"));
        assert!(text.contains("/channelops 1 — предыдущая"));
        assert!(!text.contains("/channelops 3"));
    }

    #[test]
    fn pending_listing_handles_empty_and_out_of_range_pages() {
        assert_eq!(
            render_pending(&PendingPage {
                operations: Vec::new(),
                total: 0,
                page: 1,
            }),
            "Незавершённых операций с каналами нет."
        );
        let text = render_pending(&PendingPage {
            operations: Vec::new(),
            total: PENDING_PAGE_SIZE,
            page: 3,
        });
        assert!(text.contains("На странице 3 незавершённых операций нет"));
        assert!(text.contains("доступны страницы 1..=1"));
    }

    #[test]
    fn reason_preview_strips_html_specials_and_truncates() {
        let preview = reason_preview(&format!("{}😀", "<&>\" причина ".repeat(200)));
        assert!(!preview.contains(['<', '>', '&', '"']));
        assert!(preview.ends_with('…'));
        assert_eq!(reason_preview("  обычная\nпричина  "), "обычная причина");
    }
}
