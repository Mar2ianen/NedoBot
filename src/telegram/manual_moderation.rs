use std::collections::HashSet;

use chrono::{Duration as ChronoDuration, Utc};
use sqlx::PgPool;
use teloxide::{
    prelude::*,
    types::{ChatFullInfoKind, ChatFullInfoPublicKind, ChatPermissions},
};

use crate::{
    features::manual_moderation::{
        self, ActionRecord, UndoClaim,
        types::{
            CommandKind, MAX_TARGETS_PER_BATCH, ParsedCommand, WARNING_MUTE_DURATION, parse_command,
        },
    },
    state::AppState,
    telegram::{html::Html, render::send_html_reply},
};

#[derive(Debug, Clone, Copy)]
struct Target {
    user_id: i64,
}

#[derive(Clone, Copy)]
struct BatchContext<'a> {
    bot: &'a teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &'a PgPool,
    batch_id: i64,
    chat_id: ChatId,
    actor_id: i64,
}

pub async fn handle(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    kind: CommandKind,
    args: &str,
) -> ResponseResult<()> {
    let parsed = match parse_command(kind, args) {
        Ok(parsed) => parsed,
        Err(error) => {
            send_reply(bot, msg, &format!("{error}. Причина необязательна; используй `-- причина`, если нужно её записать.")).await?;
            return Ok(());
        }
    };
    let Some(actor) = msg.from.as_ref().filter(|user| !user.is_bot) else {
        send_reply(
            bot,
            msg,
            "Команда должна быть отправлена администратором от личного аккаунта.",
        )
        .await?;
        return Ok(());
    };
    let actor_id = actor.id.0 as i64;
    if !authorize_actor(bot, msg.chat.id, actor.id, kind.requires_restrict_right()).await {
        send_reply(bot, msg, "Недостаточно прав: проверь права администратора и разрешение бота ограничивать участников.").await?;
        return Ok(());
    }

    let target_ids = match resolve_target_ids(&state.pool, msg, &parsed).await {
        Ok(ids) => ids,
        Err(error) => {
            send_reply(bot, msg, &error.to_string()).await?;
            return Ok(());
        }
    };
    let target_ids = if matches!(kind, CommandKind::Undo) {
        Vec::new()
    } else {
        target_ids
    };
    if kind.mutates() && !matches!(kind, CommandKind::Undo) && target_ids.is_empty() {
        send_reply(
            bot,
            msg,
            "Укажи цель по reply, Telegram ID или известному @username.",
        )
        .await?;
        return Ok(());
    }

    let targets = if matches!(
        kind,
        CommandKind::Mute
            | CommandKind::Ban
            | CommandKind::Warn
            | CommandKind::Unmute
            | CommandKind::Unban
    ) {
        match preflight_targets(bot, &state.pool, msg.chat.id, kind, &target_ids).await {
            Ok(targets) => targets,
            Err(error) => {
                send_reply(bot, msg, &error.to_string()).await?;
                return Ok(());
            }
        }
    } else {
        target_ids
    };

    if matches!(kind, CommandKind::Warns | CommandKind::Modlog) {
        let result = if kind == CommandKind::Warns {
            render_warnings(&state.pool, msg.chat.id.0, &targets).await
        } else {
            render_modlog(&state.pool, msg.chat.id.0, &targets, parsed.limit).await
        };
        match result {
            Ok(text) => {
                send_reply(bot, msg, &text).await?;
            }
            Err(error) => {
                tracing::error!(%error, command = kind.as_str(), "failed to read moderation history");
                send_reply(bot, msg, "Не удалось прочитать журнал модерации.").await?;
            }
        }
        return Ok(());
    }

    let batch: manual_moderation::BatchCreation = match manual_moderation::create_batch(
        &state.pool,
        msg.chat.id.0,
        actor_id,
        msg.id.0,
        kind.as_str(),
    )
    .await
    {
        Ok(batch) => batch,
        Err(error) => {
            tracing::error!(%error, "failed to create manual moderation batch");
            send_reply(
                bot,
                msg,
                "Не удалось записать команду модерации. Попробуй позже.",
            )
            .await?;
            return Ok(());
        }
    };
    if !batch.is_new {
        send_reply(
            bot,
            msg,
            "Эта команда уже обработана; повторно наказание не применялось.",
        )
        .await?;
        return Ok(());
    }
    let context = BatchContext {
        bot,
        pool: &state.pool,
        batch_id: batch.id,
        chat_id: msg.chat.id,
        actor_id,
    };

    let result = match kind {
        CommandKind::Mute | CommandKind::Ban => {
            apply_batch_restriction(
                context,
                &targets,
                kind,
                parsed.duration,
                parsed.reason.as_deref(),
                false,
            )
            .await
        }
        CommandKind::Warn => {
            apply_batch_warnings(
                context,
                &targets,
                parsed
                    .duration
                    .unwrap_or(crate::features::manual_moderation::types::WARNING_TTL),
                parsed.reason.as_deref(),
            )
            .await
        }
        CommandKind::Unmute | CommandKind::Unban => {
            revoke_restrictions(
                context,
                &targets,
                if kind == CommandKind::Unmute {
                    "mute"
                } else {
                    "ban"
                },
                parsed.reason.as_deref(),
            )
            .await
        }
        CommandKind::Unwarn => {
            revoke_warning_batch(
                &state.pool,
                batch.id,
                msg.chat.id.0,
                actor_id,
                &targets,
                parsed.warn_id,
                parsed.reason.as_deref(),
            )
            .await
        }
        CommandKind::Warns => render_warnings(&state.pool, msg.chat.id.0, &targets).await,
        CommandKind::Modlog => {
            render_modlog(&state.pool, msg.chat.id.0, &targets, parsed.limit).await
        }
        CommandKind::Undo => {
            undo_latest_batch(bot, &state.pool, batch.id, msg.chat.id, actor_id).await
        }
    };

    match result {
        Ok(text) => {
            send_reply(bot, msg, &text).await?;
        }
        Err(error) => {
            tracing::error!(%error, command = kind.as_str(), "manual moderation command failed");
            send_reply(
                bot,
                msg,
                "Не удалось завершить команду модерации. Проверь /modlog и попробуй позже.",
            )
            .await?;
        }
    }
    Ok(())
}

async fn authorize_actor(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    actor_id: UserId,
    requires_restrict_right: bool,
) -> bool {
    let Ok(actor_member) = bot.get_chat_member(chat_id, actor_id).await else {
        return false;
    };
    if !actor_member.kind.is_privileged() {
        return false;
    }
    if !requires_restrict_right {
        return true;
    }
    if !actor_member.kind.can_restrict_members() {
        return false;
    }

    let Ok(bot_user) = bot.get_me().await else {
        return false;
    };
    bot.get_chat_member(chat_id, bot_user.id)
        .await
        .is_ok_and(|member| member.kind.can_restrict_members())
}

async fn resolve_target_ids(
    pool: &PgPool,
    msg: &Message,
    parsed: &ParsedCommand,
) -> anyhow::Result<Vec<Target>> {
    let mut candidates = parsed.targets.clone();
    if let Some(reply) = msg.reply_to_message() {
        if reply.chat.id != msg.chat.id {
            anyhow::bail!("reply должен быть из этого же чата");
        }
        let user = reply
            .from
            .as_ref()
            .filter(|user| !user.is_bot)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "не удалось определить автора reply; анонимные sender_chat не поддерживаются"
                )
            })?;
        candidates.push(user.id.0.to_string());
    }

    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for candidate in candidates {
        let user_id = if let Some(username) = candidate.strip_prefix('@') {
            manual_moderation::find_username(pool, msg.chat.id.0, username)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!("@{username} не найден среди известных участников этого чата")
                })?
        } else {
            candidate
                .parse::<i64>()
                .map_err(|_| anyhow::anyhow!("цель должна быть Telegram ID или @username"))?
        };
        if user_id <= 0 {
            anyhow::bail!("Telegram ID должен быть положительным");
        }
        if seen.insert(user_id) {
            targets.push(Target { user_id });
        }
    }
    if targets.len() > MAX_TARGETS_PER_BATCH {
        anyhow::bail!("за один раз можно указать не больше {MAX_TARGETS_PER_BATCH} пользователей");
    }
    Ok(targets)
}

async fn preflight_targets(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    chat_id: ChatId,
    kind: CommandKind,
    target_ids: &[Target],
) -> anyhow::Result<Vec<Target>> {
    let mut targets = Vec::with_capacity(target_ids.len());
    for candidate in target_ids {
        let member = bot
            .get_chat_member(chat_id, UserId(candidate.user_id as u64))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "не удалось проверить статус пользователя {} в чате",
                    candidate.user_id
                )
            })?;
        if member.user.is_bot || member.kind.is_privileged() {
            anyhow::bail!("ботов и администраторов нельзя включать в ручное наказание");
        }
        let is_member = member.kind.is_present();
        let is_banned = member.kind.is_banned();
        if matches!(kind, CommandKind::Mute | CommandKind::Warn) && (!is_member || is_banned) {
            anyhow::bail!(
                "mute и warn доступны только текущим участникам чата; id {} не участник",
                candidate.user_id
            );
        }
        if kind == CommandKind::Ban && !is_member && !is_banned {
            let known =
                manual_moderation::known_chat_user(pool, chat_id.0, candidate.user_id).await?;
            if !known {
                anyhow::bail!(
                    "нельзя забанить неизвестный чату ID {}; нужен известный участник",
                    candidate.user_id
                );
            }
        }
        targets.push(Target {
            user_id: candidate.user_id,
        });
    }
    Ok(targets)
}

async fn apply_batch_restriction(
    context: BatchContext<'_>,
    targets: &[Target],
    kind: CommandKind,
    duration: Option<std::time::Duration>,
    reason: Option<&str>,
    automatic: bool,
) -> anyhow::Result<String> {
    let BatchContext {
        bot,
        pool,
        batch_id,
        chat_id,
        actor_id,
    } = context;
    let action = if automatic {
        "auto_mute"
    } else {
        match kind {
            CommandKind::Mute => "mute",
            CommandKind::Ban => "ban",
            _ => "auto_mute",
        }
    };
    let mut outcomes = Vec::new();
    for target in targets {
        let expires_at = duration.map(|duration| {
            Utc::now() + ChronoDuration::from_std(duration).expect("validated moderation duration")
        });
        let action_id = match manual_moderation::prepare_action(
            pool,
            manual_moderation::ActionPreparation {
                batch_id,
                chat_id: chat_id.0,
                target_user_id: target.user_id,
                actor_user_id: actor_id,
                action,
                reason,
                expires_at,
                automatic,
            },
        )
        .await
        {
            Ok(Some(action_id)) => action_id,
            Ok(None) => {
                outcomes.push(format!("{} — ограничен уже", target.user_id));
                continue;
            }
            Err(error) => {
                outcomes.push(format!(
                    "{} — не выполнено: {}",
                    target.user_id,
                    safe_error(&error)
                ));
                continue;
            }
        };
        let request = if kind == CommandKind::Ban {
            apply_ban(bot, chat_id, target.user_id, expires_at).await
        } else {
            apply_mute(bot, chat_id, target.user_id, expires_at).await
        };
        match request {
            Ok(()) => match manual_moderation::mark_action_succeeded(pool, action_id).await {
                Ok(()) => outcomes.push(format!(
                    "{} — {}",
                    target.user_id,
                    action_label(action, expires_at)
                )),
                Err(error) => {
                    tracing::error!(%error, target_user_id = target.user_id, "Telegram action succeeded but audit finalization failed");
                    outcomes.push(format!(
                        "{} — применено в Telegram, запись требует сверки",
                        target.user_id
                    ));
                }
            },
            Err(error) => {
                let unknown = telegram_outcome_unknown(&error);
                if let Err(store_error) = manual_moderation::mark_action_failed(
                    pool,
                    action_id,
                    unknown,
                    if unknown {
                        "telegram outcome uncertain"
                    } else {
                        "telegram rejected action"
                    },
                )
                .await
                {
                    tracing::error!(%store_error, target_user_id = target.user_id, "failed to persist moderation outcome");
                }
                outcomes.push(format!(
                    "{} — {}",
                    target.user_id,
                    if unknown {
                        "результат Telegram неизвестен"
                    } else {
                        "Telegram отклонил действие"
                    }
                ));
            }
        }
    }
    Ok(outcomes.join("\n"))
}

async fn apply_batch_warnings(
    context: BatchContext<'_>,
    targets: &[Target],
    ttl: std::time::Duration,
    reason: Option<&str>,
) -> anyhow::Result<String> {
    let BatchContext {
        pool,
        batch_id,
        chat_id,
        actor_id,
        ..
    } = context;
    let mut outcomes = Vec::new();
    for target in targets {
        let warning: manual_moderation::WarningResult = manual_moderation::add_warning(
            pool,
            batch_id,
            chat_id.0,
            target.user_id,
            actor_id,
            ttl,
            reason,
        )
        .await?;
        let escalation = if warning.should_escalate {
            let mute = apply_batch_restriction(
                context,
                &[*target],
                CommandKind::Mute,
                Some(WARNING_MUTE_DURATION),
                Some("автоматически: три активных предупреждения"),
                true,
            )
            .await?;
            format!("; порог 3 предупреждения — {mute}")
        } else {
            String::new()
        };
        outcomes.push(format!(
            "{} — предупреждение №{} ({}/3 активных){}",
            target.user_id, warning.action_id, warning.active_count, escalation
        ));
    }
    Ok(outcomes.join("\n"))
}

async fn revoke_restrictions(
    context: BatchContext<'_>,
    targets: &[Target],
    expected_action: &str,
    reason: Option<&str>,
) -> anyhow::Result<String> {
    let BatchContext {
        bot,
        pool,
        batch_id,
        chat_id,
        actor_id,
    } = context;
    let mut outcomes = Vec::new();
    for target in targets {
        let Some(action) = manual_moderation::claim_restriction_revoke(
            pool,
            batch_id,
            chat_id.0,
            target.user_id,
            actor_id,
            expected_action,
        )
        .await?
        else {
            outcomes.push(format!(
                "{} — активная мера бота не найдена",
                target.user_id
            ));
            continue;
        };
        match clear_restriction(bot, chat_id, target.user_id, &action.action).await {
            Ok(()) => {
                manual_moderation::finish_restriction_revoke(
                    pool, batch_id, action.id, actor_id, true, false, reason,
                )
                .await?;
                outcomes.push(format!("{} — ограничение снято", target.user_id));
            }
            Err(error) => {
                let unknown = telegram_outcome_unknown(&error);
                manual_moderation::finish_restriction_revoke(
                    pool, batch_id, action.id, actor_id, false, unknown, reason,
                )
                .await?;
                outcomes.push(format!(
                    "{} — {}",
                    target.user_id,
                    if unknown {
                        "результат Telegram неизвестен"
                    } else {
                        "Telegram отклонил снятие"
                    }
                ));
            }
        }
    }
    Ok(outcomes.join("\n"))
}

async fn revoke_warning_batch(
    pool: &PgPool,
    batch_id: i64,
    chat_id: i64,
    actor_id: i64,
    targets: &[Target],
    warn_id: Option<i64>,
    reason: Option<&str>,
) -> anyhow::Result<String> {
    let mut outcomes = Vec::new();
    for target in targets {
        let revoked = manual_moderation::revoke_warnings(
            pool,
            batch_id,
            chat_id,
            target.user_id,
            actor_id,
            warn_id,
            reason,
        )
        .await?;
        outcomes.push(if revoked.is_empty() {
            format!("{} — активных предупреждений не найдено", target.user_id)
        } else {
            format!(
                "{} — отозваны предупреждения {}",
                target.user_id,
                revoked
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
    }
    Ok(outcomes.join("\n"))
}

async fn render_warnings(
    pool: &PgPool,
    chat_id: i64,
    targets: &[Target],
) -> anyhow::Result<String> {
    let mut lines = Vec::new();
    for target in targets {
        let warnings = manual_moderation::list_warnings(pool, chat_id, target.user_id, 20).await?;
        lines.push(format!(
            "{} — активных: {}",
            target.user_id,
            manual_moderation::active_warning_count(pool, chat_id, target.user_id).await?
        ));
        for warning in warnings {
            lines.push(format_action(&warning));
        }
    }
    Ok(if lines.is_empty() {
        "Укажи цель reply, ID или известным @username.".to_string()
    } else {
        lines.join("\n")
    })
}

async fn render_modlog(
    pool: &PgPool,
    chat_id: i64,
    targets: &[Target],
    limit: i64,
) -> anyhow::Result<String> {
    let target = targets.first().map(|target| target.user_id);
    let actions = manual_moderation::list_actions(pool, chat_id, target, limit).await?;
    Ok(if actions.is_empty() {
        "Записей модерации нет.".to_string()
    } else {
        actions
            .iter()
            .map(format_action)
            .collect::<Vec<_>>()
            .join("\n")
    })
}

async fn undo_latest_batch(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    undo_batch_id: i64,
    chat_id: ChatId,
    actor_id: i64,
) -> anyhow::Result<String> {
    let Some(batch_id) = manual_moderation::latest_batch(pool, chat_id.0, actor_id).await? else {
        return Ok("Нет ранее применённых команд модерации для отмены.".to_string());
    };
    let actions = manual_moderation::actions_in_batch(pool, batch_id).await?;
    let mut outcomes = Vec::new();
    let mut checked_targets = HashSet::new();
    for action in &actions {
        if !checked_targets.insert(action.target_user_id) {
            continue;
        }
        let member = match bot
            .get_chat_member(chat_id, UserId(action.target_user_id as u64))
            .await
        {
            Ok(member) => member,
            Err(_) => {
                return Ok(format!(
                    "Отмена не выполнена: не удалось проверить пользователя {}; ни одно действие не изменено.",
                    action.target_user_id
                ));
            }
        };
        if member.user.is_bot || member.kind.is_privileged() {
            return Ok(format!(
                "Отмена не выполнена: пользователь {} теперь бот или администратор; ни одно действие не изменено.",
                action.target_user_id
            ));
        }
    }
    for action in actions {
        match manual_moderation::claim_undo_action(
            pool,
            undo_batch_id,
            batch_id,
            action.id,
            actor_id,
        )
        .await?
        {
            UndoClaim::WarningRevoked => outcomes.push(format!("{} — warn #{} отменён", action.target_user_id, action.id)),
            UndoClaim::WarningRestored => outcomes.push(format!("{} — warn #{} восстановлен", action.target_user_id, action.id)),
            UndoClaim::Conflict => outcomes.push(format!("{} — пропуск: после команды появилось более новое действие или исход не подтверждён", action.target_user_id)),
            UndoClaim::Expired => outcomes.push(format!("{} — пропуск: мера уже истекла", action.target_user_id)),
            claim @ (UndoClaim::RestrictionClaimed | UndoClaim::RestrictionReapplyClaimed) => {
                let reapply = claim == UndoClaim::RestrictionReapplyClaimed;
                let previous = manual_moderation::undo_previous_action(pool, action.id).await?;
                let result = if reapply {
                    reapply_action(bot, chat_id, &action)
                        .await
                        .map_err(|error| telegram_outcome_unknown(&error))
                } else {
                    restore_action(bot, chat_id, &action, previous.as_ref()).await
                };
                let success = result.is_ok();
                let unknown = result.as_ref().is_err_and(|unknown| *unknown);
                manual_moderation::finish_undo(
                    pool,
                    undo_batch_id,
                    action.id,
                    actor_id,
                    success,
                    unknown,
                    reapply,
                )
                .await?;
                outcomes.push(format!(
                    "{} — {}",
                    action.target_user_id,
                    if success {
                        if reapply {
                            if action.action == "ban" { "бан восстановлен" } else { "мут восстановлен" }
                        } else {
                            previous.as_ref().map_or("ограничение снято", |old| if old.action == "ban" { "предыдущий бан восстановлен" } else { "предыдущий mute восстановлен" })
                        }
                    } else if unknown {
                        "результат Telegram неизвестен"
                    } else {
                        if reapply { "Telegram отклонил восстановление" } else { "Telegram отклонил отмену" }
                    }
                ));
            }
        }
    }
    Ok(if outcomes.is_empty() {
        "В отменяемой команде не осталось активных действий.".to_string()
    } else {
        outcomes.join("\n")
    })
}

async fn apply_mute(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    user_id: i64,
    expires_at: Option<chrono::DateTime<Utc>>,
) -> Result<(), teloxide::RequestError> {
    let request = bot
        .restrict_chat_member(chat_id, UserId(user_id as u64), ChatPermissions::empty())
        .use_independent_chat_permissions(true);
    match expires_at {
        Some(expires_at) => request.until_date(expires_at).await.map(|_| ()),
        None => request.await.map(|_| ()),
    }
}

async fn apply_ban(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    user_id: i64,
    expires_at: Option<chrono::DateTime<Utc>>,
) -> Result<(), teloxide::RequestError> {
    let request = bot.ban_chat_member(chat_id, UserId(user_id as u64));
    match expires_at {
        Some(expires_at) => request.until_date(expires_at).await.map(|_| ()),
        None => request.await.map(|_| ()),
    }
}

async fn clear_restriction(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    user_id: i64,
    action: &str,
) -> Result<(), teloxide::RequestError> {
    if action == "ban" {
        bot.unban_chat_member(chat_id, UserId(user_id as u64))
            .await
            .map(|_| ())
    } else {
        let permissions = default_chat_permissions(bot, chat_id).await?;
        bot.restrict_chat_member(chat_id, UserId(user_id as u64), permissions)
            .use_independent_chat_permissions(true)
            .await
            .map(|_| ())
    }
}

async fn reapply_action(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    action: &ActionRecord,
) -> Result<(), teloxide::RequestError> {
    if action.action == "ban" {
        apply_ban(bot, chat_id, action.target_user_id, action.expires_at).await
    } else {
        apply_mute(bot, chat_id, action.target_user_id, action.expires_at).await
    }
}

async fn restore_action(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    current: &ActionRecord,
    previous: Option<&ActionRecord>,
) -> Result<(), bool> {
    let Some(previous) = previous else {
        return clear_restriction(bot, chat_id, current.target_user_id, &current.action)
            .await
            .map_err(|error| telegram_outcome_unknown(&error));
    };
    if current.action == "ban" && previous.action != "ban" {
        bot.unban_chat_member(chat_id, UserId(current.target_user_id as u64))
            .await
            .map_err(|error| telegram_outcome_unknown(&error))?;
    }
    if previous.action == "ban" {
        let request = bot.ban_chat_member(chat_id, UserId(current.target_user_id as u64));
        match previous.expires_at {
            Some(expires_at) => request
                .until_date(expires_at)
                .await
                .map(|_| ())
                .map_err(|error| telegram_outcome_unknown(&error)),
            None => request
                .await
                .map(|_| ())
                .map_err(|error| telegram_outcome_unknown(&error)),
        }
    } else {
        bot.restrict_chat_member(
            chat_id,
            UserId(current.target_user_id as u64),
            ChatPermissions::empty(),
        )
        .use_independent_chat_permissions(true)
        .until_date(previous.expires_at.ok_or(true)?)
        .await
        .map(|_| ())
        // A failed restore after unbanning has already changed Telegram state.
        .map_err(|_| true)
    }
}

async fn default_chat_permissions(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
) -> Result<ChatPermissions, teloxide::RequestError> {
    let info = bot.get_chat(chat_id).await?;
    match info.kind {
        ChatFullInfoKind::Public(public) => match public.kind {
            ChatFullInfoPublicKind::Group(group) => group.permissions,
            ChatFullInfoPublicKind::Supergroup(supergroup) => supergroup.permissions,
            ChatFullInfoPublicKind::Channel(_) => None,
        },
        ChatFullInfoKind::Private(_) => None,
    }
    .ok_or_else(|| {
        teloxide::RequestError::Io(
            std::io::Error::other("chat default permissions unavailable").into(),
        )
    })
}

fn telegram_outcome_unknown(error: &teloxide::RequestError) -> bool {
    matches!(
        error,
        teloxide::RequestError::Network(_) | teloxide::RequestError::InvalidJson { .. }
    )
}

fn action_label(action: &str, expires_at: Option<chrono::DateTime<Utc>>) -> String {
    let label = if action == "ban" {
        "бан"
    } else if action == "auto_mute" {
        "автомут"
    } else {
        "мут"
    };
    match expires_at {
        Some(expires_at) => format!("{label} до {} UTC", expires_at.format("%d.%m.%Y %H:%M")),
        None => format!("{label} навсегда"),
    }
}

fn format_action(action: &ActionRecord) -> String {
    let until = action
        .expires_at
        .map(|expires_at| expires_at.format("%d.%m.%Y %H:%M UTC").to_string())
        .unwrap_or_else(|| "навсегда".to_string());
    let reason = action
        .reason
        .as_deref()
        .map(|reason| format!(" — {}", reason))
        .unwrap_or_default();
    let source = if action.automatic {
        "авто"
    } else {
        "ручн."
    };
    format!(
        "#{} {} ({source}) → {} · {} · до {}{} · actor {}",
        action.id,
        action.action,
        action.status,
        action.created_at.format("%d.%m.%Y %H:%M UTC"),
        until,
        reason,
        action.actor_user_id
    )
}

fn safe_error(error: &anyhow::Error) -> &'static str {
    let message = error.to_string();
    if message.contains("забанен") {
        "цель уже забанена"
    } else if message.contains("неопределённый") {
        "нужна ручная сверка"
    } else if message.contains("другая команда") {
        "для цели уже идёт другая команда"
    } else {
        "внутренняя проверка не пройдена"
    }
}

async fn send_reply(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    text: &str,
) -> ResponseResult<Message> {
    send_html_reply(bot, msg.chat.id, msg.id, Html::text(text).into_string()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escalation_action_uses_fixed_five_day_expiration() {
        assert_eq!(
            WARNING_MUTE_DURATION,
            std::time::Duration::from_secs(5 * 24 * 60 * 60)
        );
    }

    #[test]
    fn reason_output_is_escaped_before_telegram_html() {
        let action = ActionRecord {
            id: 1,
            batch_id: 1,
            chat_id: -1001,
            target_user_id: 2,
            actor_user_id: 3,
            action: "warn".to_string(),
            reason: Some("<script>".to_string()),
            status: "applied".to_string(),
            created_at: Utc::now(),
            expires_at: None,
            supersedes_action_id: None,
            automatic: false,
        };
        assert!(
            Html::text(format_action(&action))
                .as_str()
                .contains("&lt;script&gt;")
        );
    }
}
