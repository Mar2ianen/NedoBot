use std::collections::HashSet;

use chrono::{Duration as ChronoDuration, Utc};
use sqlx::PgPool;
use teloxide::{
    prelude::*,
    types::{ChatFullInfoKind, ChatFullInfoPublicKind, ChatMemberKind, ChatPermissions},
};

use crate::{
    features::manual_moderation::{
        self, ActionRecord, UndoClaim,
        types::{
            BatchRequestSnapshot, BatchTarget, CommandKind, MAX_TARGETS_PER_BATCH, ParsedCommand,
            WARNING_MUTE_DURATION, parse_command,
        },
    },
    state::AppState,
    telegram::{html::Html, service_messages},
};

type Target = BatchTarget;

#[derive(Debug, Clone, Copy)]
enum ObservedRestriction {
    None,
    Restricted,
    Banned,
}

#[derive(Clone, Copy)]
struct BatchContext<'a> {
    bot: &'a teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &'a PgPool,
    batch_id: i64,
    chat_id: ChatId,
    actor_id: i64,
}

/// A Telegram actor whose moderator status and operation-specific rights were
/// verified against the Bot API for this command's chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizedActor {
    user_id: UserId,
}

pub async fn handle(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    kind: CommandKind,
    args: &str,
) -> ResponseResult<()> {
    let mut parsed = match parse_command(kind, args) {
        Ok(parsed) => parsed,
        Err(error) => {
            send_reply(bot, msg, state, &format!("{error}. Причина необязательна; используй `-- причина`, если нужно её записать.")).await?;
            return Ok(());
        }
    };
    let Some(actor) = msg.from.as_ref().filter(|user| !user.is_bot) else {
        send_reply(
            bot,
            msg,
            state,
            "Команда должна быть отправлена администратором от личного аккаунта.",
        )
        .await?;
        return Ok(());
    };
    let Some(actor) =
        authorize_actor(bot, msg.chat.id, actor.id, kind.requires_restrict_right()).await
    else {
        send_reply(bot, msg, state, "Недостаточно прав: проверь права администратора и разрешение бота ограничивать участников.").await?;
        return Ok(());
    };
    let actor_id = actor.user_id.0 as i64;
    match super::manual_sender_chat::target(msg, &parsed) {
        Ok(Some((action, sender))) => {
            if !matches!(
                manual_moderation::find_batch(&state.pool, msg.chat.id.0, msg.id.0).await,
                Ok(None)
            ) {
                send_reply(bot,msg,state,"Эта команда уже зарегистрирована для пользователя; отправь новую команду канала.").await?;
                return Ok(());
            }

            if !state
                .config
                .chat_allows(msg.chat.id.0, |chat| chat.ingest && chat.moderation)
            {
                send_reply(bot, msg, state, "Модерация этого чата не включена.").await?;
                return Ok(());
            }
            let result = super::manual_sender_chat::execute(
                bot,
                &state.pool,
                msg,
                actor.user_id,
                action,
                &sender,
                parsed.reason.as_deref(),
            )
            .await;
            let text = match result {
                Ok(text) => text,
                Err(error) => {
                    tracing::error!(%error, "manual channel moderation failed");
                    "Не удалось завершить команду канала. Проверь журнал и ограничения Telegram; повтор автоматически не выполняется.".into()
                }
            };
            send_reply(bot, msg, state, &text).await?;
            return Ok(());
        }
        Err(error) => {
            send_reply(bot, msg, state, &error.to_string()).await?;
            return Ok(());
        }
        Ok(None) => {
            match manual_moderation::sender_chat::command_exists(
                &state.pool,
                msg.chat.id.0,
                msg.id.0,
            )
            .await
            {
                Ok(false) => {}
                _ => {
                    send_reply(bot,msg,state,"Команда канала не может быть изменена на пользовательскую; отправь новую команду.").await?;
                    return Ok(());
                }
            }
        }
    }

    if matches!(kind, CommandKind::Warns | CommandKind::Modlog) {
        let targets = match resolve_target_ids(&state.pool, msg, &parsed).await {
            Ok(targets) => targets,
            Err(error) => {
                send_reply(bot, msg, state, &error.to_string()).await?;
                return Ok(());
            }
        };
        let result = if kind == CommandKind::Warns {
            render_warnings(&state.pool, msg.chat.id.0, &targets).await
        } else {
            render_modlog(&state.pool, msg.chat.id.0, &targets, parsed.limit).await
        };
        match result {
            Ok(text) => {
                send_reply(bot, msg, state, &text).await?;
            }
            Err(error) => {
                tracing::error!(%error, command = kind.as_str(), "failed to read moderation history");
                send_reply(bot, msg, state, "Не удалось прочитать журнал модерации.").await?;
            }
        }
        return Ok(());
    }

    let existing = match manual_moderation::find_batch(&state.pool, msg.chat.id.0, msg.id.0).await {
        Ok(batch) => batch,
        Err(error) => {
            tracing::error!(%error, "failed to read moderation batch");
            send_reply(
                bot,
                msg,
                state,
                "Не удалось прочитать состояние команды модерации.",
            )
            .await?;
            return Ok(());
        }
    };
    let (targets, batch) = if let Some(batch) = existing {
        if batch.actor_user_id != actor_id || batch.command != kind.as_str() {
            send_reply(
                bot,
                msg,
                state,
                "Эта команда уже зарегистрирована с другим автором или типом.",
            )
            .await?;
            return Ok(());
        }
        if batch.status == "completed" {
            send_reply(
                bot,
                msg,
                state,
                batch
                    .result_text
                    .as_deref()
                    .unwrap_or("Команда уже завершена."),
            )
            .await?;
            return Ok(());
        }
        let Some(snapshot) = batch.request.as_ref() else {
            send_reply(
                bot,
                msg,
                state,
                batch.result_text.as_deref().unwrap_or(
                    "Старую команду нельзя безопасно продолжить; проверь /modlog и Telegram.",
                ),
            )
            .await?;
            return Ok(());
        };
        if snapshot.kind != kind {
            send_reply(
                bot,
                msg,
                state,
                "Сохранённый тип команды не совпадает; действие остановлено.",
            )
            .await?;
            return Ok(());
        }
        if let Err(error) =
            manual_moderation::recover_expired_batch_actions(&state.pool, batch.id).await
        {
            tracing::error!(%error, batch_id = batch.id, "failed to recover stale moderation operations");
            send_reply(
                bot,
                msg,
                state,
                "Не удалось восстановить состояние команды. Проверь Telegram и попробуй позже.",
            )
            .await?;
            return Ok(());
        }
        parsed = snapshot.parsed_command();
        let ids = match snapshot.resume_targets() {
            Ok(targets) => targets,
            Err(error) => {
                send_reply(bot, msg, state, &error).await?;
                return Ok(());
            }
        };
        let targets =
            match maybe_check_targets(bot, &state.pool, msg.chat.id, kind, ids, Some(batch.id))
                .await
            {
                Ok(targets) => targets,
                Err(error) => {
                    send_reply(bot, msg, state, &error.to_string()).await?;
                    return Ok(());
                }
            };
        (targets, batch)
    } else {
        let ids = match resolve_target_ids(&state.pool, msg, &parsed).await {
            Ok(ids) => ids,
            Err(error) => {
                send_reply(bot, msg, state, &error.to_string()).await?;
                return Ok(());
            }
        };
        let ids = if kind == CommandKind::Undo {
            Vec::new()
        } else {
            ids
        };
        if kind.mutates() && kind != CommandKind::Undo && ids.is_empty() {
            send_reply(
                bot,
                msg,
                state,
                "Укажи цель по reply, Telegram ID или известному @username.",
            )
            .await?;
            return Ok(());
        }
        let targets =
            match maybe_check_targets(bot, &state.pool, msg.chat.id, kind, ids, None).await {
                Ok(targets) => targets,
                Err(error) => {
                    send_reply(bot, msg, state, &error.to_string()).await?;
                    return Ok(());
                }
            };
        let request = BatchRequestSnapshot::from_parsed(&parsed, targets.clone());
        let batch = match manual_moderation::create_batch_with_request(
            &state.pool,
            msg.chat.id.0,
            actor_id,
            msg.id.0,
            kind.as_str(),
            &request,
        )
        .await
        {
            Ok(batch) => batch,
            Err(error) => {
                tracing::error!(%error, "failed to create manual moderation batch");
                send_reply(
                    bot,
                    msg,
                    state,
                    "Не удалось записать команду модерации. Попробуй позже.",
                )
                .await?;
                return Ok(());
            }
        };
        (targets, batch)
    };

    match manual_moderation::claim_batch(&state.pool, batch.id).await {
        Ok(manual_moderation::BatchClaim::Claimed) => {}
        Ok(manual_moderation::BatchClaim::Busy) => {
            send_reply(
                bot,
                msg,
                state,
                "Команда уже выполняется; повторно действие не запускалось.",
            )
            .await?;
            return Ok(());
        }
        Ok(manual_moderation::BatchClaim::Finished(result)) => {
            send_reply(
                bot,
                msg,
                state,
                result
                    .as_deref()
                    .unwrap_or("Команда завершена; повторно действие не запускалось."),
            )
            .await?;
            return Ok(());
        }
        Err(error) => {
            tracing::error!(%error, batch_id = batch.id, "failed to claim moderation batch");
            send_reply(
                bot,
                msg,
                state,
                "Не удалось получить команду для выполнения.",
            )
            .await?;
            return Ok(());
        }
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
                if parsed.warn_all {
                    manual_moderation::WarningSelection::All
                } else if let Some(warn_id) = parsed.warn_id {
                    manual_moderation::WarningSelection::Id(warn_id)
                } else {
                    manual_moderation::WarningSelection::Latest
                },
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
            if let Err(error) = manual_moderation::finish_batch(&state.pool, batch.id, &text).await
            {
                tracing::error!(%error, batch_id = batch.id, "failed to save moderation batch result");
                send_reply(bot, msg, state, "Команда выполнена частично, но результат не удалось сохранить; проверь /modlog и Telegram.").await?;
                return Ok(());
            }
            send_reply(bot, msg, state, &text).await?;
            if matches!(
                kind,
                CommandKind::Mute | CommandKind::Ban | CommandKind::Warn
            ) {
                match manual_moderation::actions_in_batch(&state.pool, batch.id).await {
                    Ok(actions) if should_delete_moderation_messages(kind, &actions) => {
                        delete_configured_messages(bot, msg, state).await;
                    }
                    Ok(_) => {}
                    Err(error) => tracing::error!(
                        %error,
                        batch_id = batch.id,
                        "failed to check whether moderation message deletion is allowed"
                    ),
                }
            }
        }
        Err(error) => {
            tracing::error!(%error, command = kind.as_str(), "manual moderation command failed");
            send_reply(
                bot,
                msg,
                state,
                "Не удалось завершить команду модерации. Проверь /modlog и попробуй позже.",
            )
            .await?;
        }
    }
    Ok(())
}

fn should_delete_moderation_messages(kind: CommandKind, actions: &[ActionRecord]) -> bool {
    matches!(
        kind,
        CommandKind::Mute | CommandKind::Ban | CommandKind::Warn
    ) && actions.iter().any(|action| {
        action.status == "applied"
            && matches!(
                action.action.as_str(),
                "mute" | "ban" | "warn" | "auto_mute"
            )
    })
}

async fn delete_configured_messages(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
) {
    let Some(chat) = state.config.chat_by_id(msg.chat.id.0) else {
        return;
    };
    if chat.config.moderation_delete_target_message
        && let Some(target) = msg.reply_to_message()
        && let Err(error) = bot.delete_message(msg.chat.id, target.id).await
    {
        tracing::warn!(
            %error,
            chat_id = msg.chat.id.0,
            target_message_id = target.id.0,
            "failed to delete configured moderation target message"
        );
    }
    if chat.config.moderation_delete_command_message
        && let Err(error) = bot.delete_message(msg.chat.id, msg.id).await
    {
        tracing::warn!(
            %error,
            chat_id = msg.chat.id.0,
            command_message_id = msg.id.0,
            "failed to delete configured moderation command message"
        );
    }
}

async fn authorize_actor(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    actor_id: UserId,
    requires_restrict_right: bool,
) -> Option<AuthorizedActor> {
    let Ok(actor_member) = bot.get_chat_member(chat_id, actor_id).await else {
        return None;
    };
    let actor_privileged = actor_member.kind.is_privileged();
    let actor_can_restrict = actor_member.kind.can_restrict_members();
    // The bot's rights are checked separately below; don't treat an
    // observation we haven't made yet as an explicit denial.
    if !actor_privileged || (requires_restrict_right && !actor_can_restrict) {
        return None;
    }
    if !requires_restrict_right {
        return Some(AuthorizedActor { user_id: actor_id });
    }

    let Ok(bot_user) = bot.get_me().await else {
        return None;
    };
    bot.get_chat_member(chat_id, bot_user.id)
        .await
        .ok()
        .filter(|member| {
            actor_rights_allow_action(
                actor_privileged,
                actor_can_restrict,
                member.kind.can_restrict_members(),
                requires_restrict_right,
            )
        })
        .map(|_| AuthorizedActor { user_id: actor_id })
}

fn actor_rights_allow_action(
    actor_is_privileged: bool,
    actor_can_restrict: bool,
    bot_can_restrict: bool,
    requires_restrict_right: bool,
) -> bool {
    actor_is_privileged && (!requires_restrict_right || (actor_can_restrict && bot_can_restrict))
}

async fn resolve_target_ids(
    pool: &PgPool,
    msg: &Message,
    parsed: &ParsedCommand,
) -> anyhow::Result<Vec<Target>> {
    let reply_user_id = if let Some(reply) = msg.reply_to_message() {
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
        Some(user.id.0 as i64)
    } else {
        None
    };
    resolve_target_requests(pool, msg.chat.id.0, &parsed.targets, reply_user_id).await
}

async fn resolve_target_requests(
    pool: &PgPool,
    chat_id: i64,
    candidates: &[String],
    reply_user_id: Option<i64>,
) -> anyhow::Result<Vec<Target>> {
    let mut targets: Vec<Target> = Vec::new();
    for candidate in candidates {
        let target = if let Some(username) = candidate.strip_prefix('@') {
            let username = username.to_ascii_lowercase();
            let user_id = manual_moderation::find_username(pool, chat_id, &username)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!("@{username} не найден среди известных участников этого чата")
                })?;
            Target::username(user_id, username)
        } else {
            let user_id = candidate
                .parse::<i64>()
                .map_err(|_| anyhow::anyhow!("цель должна быть Telegram ID или @username"))?;
            Target::numeric(user_id)
        };
        anyhow::ensure!(
            !crate::telegram::identity::is_channel_sender_proxy(target.user_id),
            "Это технический отправитель канала. Используй /ban или /unban по reply на комментарий канала."
        );
        if target.user_id <= 0 {
            anyhow::bail!("Telegram ID должен быть положительным");
        }
        merge_target(&mut targets, target)?;
    }
    if let Some(user_id) = reply_user_id {
        if user_id <= 0 {
            anyhow::bail!("Telegram ID должен быть положительным");
        }
        merge_target(&mut targets, Target::reply(user_id))?;
    }
    if targets.len() > MAX_TARGETS_PER_BATCH {
        anyhow::bail!("за один раз можно указать не больше {MAX_TARGETS_PER_BATCH} пользователей");
    }
    Ok(targets)
}

fn merge_target(targets: &mut Vec<Target>, incoming: Target) -> anyhow::Result<()> {
    if let Some(existing) = targets
        .iter_mut()
        .find(|existing| existing.user_id == incoming.user_id)
    {
        // A username in the same command is the stricter provenance: keep the
        // live ownership check even if the same numeric ID was also given.
        if incoming.requested_username().is_some() {
            if existing.requested_username().is_some()
                && existing.requested_username() != incoming.requested_username()
            {
                anyhow::bail!(
                    "для одного пользователя указаны разные username; используй reply или Telegram ID"
                );
            }
            *existing = incoming;
        }
        return Ok(());
    }
    targets.push(incoming);
    Ok(())
}

async fn checked_targets(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    chat_id: ChatId,
    kind: CommandKind,
    target_ids: &[Target],
    batch_id: Option<i64>,
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
        verify_live_username(&member.user, candidate)?;
        let local_restriction =
            manual_moderation::active_restriction_action(pool, chat_id.0, candidate.user_id)
                .await?;
        let observed_restriction = observed_restriction(&member.kind);
        let uncertain_for_this_batch = if let Some(batch_id) = batch_id {
            manual_moderation::batch_has_uncertain_target(pool, batch_id, candidate.user_id).await?
        } else {
            false
        };
        if !uncertain_for_this_batch {
            validate_observed_restriction(
                observed_restriction,
                local_restriction.as_deref(),
                candidate.user_id,
            )?;
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
        targets.push(candidate.clone());
    }
    Ok(targets)
}

/// A username target is only the candidate lookup key. The effect boundary
/// requires Telegram itself to still report that exact current username for the
/// resolved numeric ID; missing, reassigned, or unavailable names fail closed.
fn verify_live_username(member: &teloxide::types::User, target: &Target) -> anyhow::Result<()> {
    let Some(requested) = target.requested_username() else {
        return Ok(());
    };
    let live = member.username.as_deref().map(str::to_ascii_lowercase);
    if member.id.0 != target.user_id as u64 || live.as_deref() != Some(requested) {
        anyhow::bail!(
            "@{requested} больше не подтверждается Telegram для пользователя {}; укажи reply или Telegram ID, если нужен именно этот участник",
            target.user_id
        );
    }
    Ok(())
}

/// Re-checks username provenance immediately before a Telegram effect. The
/// earlier preflight is not enough: a warn escalation or a resumed batch can
/// reach the effect boundary after an arbitrary delay.
async fn reverify_live_usernames(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    targets: &[Target],
) -> anyhow::Result<()> {
    for target in targets {
        if target.requested_username().is_none() {
            continue;
        }
        let member = bot
            .get_chat_member(chat_id, UserId(target.user_id as u64))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "не удалось повторно проверить @{} для пользователя {}; действие остановлено",
                    target.requested_username().unwrap_or(""),
                    target.user_id
                )
            })?;
        verify_live_username(&member.user, target)?;
        if member.user.is_bot || member.kind.is_privileged() {
            anyhow::bail!("ботов и администраторов нельзя включать в ручное наказание");
        }
    }
    Ok(())
}

fn validate_observed_restriction(
    observed: ObservedRestriction,
    local_restriction: Option<&str>,
    user_id: i64,
) -> anyhow::Result<()> {
    match observed {
        ObservedRestriction::Restricted
            if !matches!(local_restriction, Some("mute" | "auto_mute")) =>
        {
            anyhow::bail!(
                "у пользователя {user_id} есть ограничение Telegram, которого нет в журнале бота; действие остановлено"
            );
        }
        ObservedRestriction::Banned if local_restriction != Some("ban") => {
            anyhow::bail!(
                "у пользователя {user_id} есть внешний бан, которого нет в журнале бота; действие остановлено"
            );
        }
        _ => {}
    }
    Ok(())
}

fn observed_restriction(kind: &ChatMemberKind) -> ObservedRestriction {
    match kind {
        ChatMemberKind::Restricted(_) => ObservedRestriction::Restricted,
        ChatMemberKind::Banned(_) => ObservedRestriction::Banned,
        _ => ObservedRestriction::None,
    }
}

async fn maybe_check_targets(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    chat_id: ChatId,
    kind: CommandKind,
    targets: Vec<Target>,
    batch_id: Option<i64>,
) -> anyhow::Result<Vec<Target>> {
    if matches!(
        kind,
        CommandKind::Mute
            | CommandKind::Ban
            | CommandKind::Warn
            | CommandKind::Unmute
            | CommandKind::Unban
    ) {
        checked_targets(bot, pool, chat_id, kind, &targets, batch_id).await
    } else if kind == CommandKind::Unwarn {
        reverify_live_usernames(bot, chat_id, &targets).await?;
        Ok(targets)
    } else {
        Ok(targets)
    }
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
    reverify_live_usernames(bot, chat_id, targets).await?;
    let expires_at = duration.map(|duration| {
        Utc::now() + ChronoDuration::from_std(duration).expect("validated moderation duration")
    });
    let preparations = targets
        .iter()
        .map(|target| manual_moderation::ActionPreparation {
            batch_id,
            chat_id: chat_id.0,
            target_user_id: target.user_id,
            actor_user_id: actor_id,
            action,
            reason,
            expires_at,
            automatic,
        })
        .collect::<Vec<_>>();
    // Reserve and validate every target in one transaction before making the
    // first Telegram call, so a later DB conflict cannot produce a partial batch.
    let prepared = manual_moderation::prepare_actions_batch(pool, &preparations).await?;
    let mut outcomes = Vec::new();
    for (target, prepared) in targets.iter().zip(prepared) {
        let Some(prepared) = prepared else {
            outcomes.push(format!("{} — ограничен уже", target.user_id));
            continue;
        };
        let action_id = prepared.id;
        let expires_at = prepared.expires_at;
        match prepared.status.as_str() {
            "applied" => {
                outcomes.push(format!(
                    "{} — {} уже применено",
                    target.user_id,
                    action_label(action, expires_at)
                ));
                continue;
            }
            "failed" => {
                outcomes.push(format!(
                    "{} — предыдущее действие Telegram отклонил",
                    target.user_id
                ));
                continue;
            }
            "unknown" | "processing" => {
                outcomes.push(format!(
                    "{} — исход действия не подтверждён; повтор запрещён",
                    target.user_id
                ));
                continue;
            }
            "pending" => {}
            status => {
                outcomes.push(format!(
                    "{} — неподдерживаемое состояние действия {status}",
                    target.user_id
                ));
                continue;
            }
        }
        if !manual_moderation::start_prepared_action(pool, action_id).await? {
            outcomes.push(format!(
                "{} — действие уже запущено другим обработчиком",
                target.user_id
            ));
            continue;
        }
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
    let warning_results = manual_moderation::add_warnings_batch(
        pool,
        batch_id,
        chat_id.0,
        &targets
            .iter()
            .map(|target| target.user_id)
            .collect::<Vec<_>>(),
        actor_id,
        ttl,
        reason,
    )
    .await?;
    let mut outcomes = Vec::new();
    let mut escalation_targets = Vec::new();
    for (target, warning) in targets.iter().zip(warning_results) {
        if warning.should_escalate {
            escalation_targets.push(target.clone());
        }
        outcomes.push(format!(
            "{} — предупреждение №{} ({}/3 активных)",
            target.user_id, warning.action_id, warning.active_count
        ));
    }
    if !escalation_targets.is_empty() {
        let mutes = apply_batch_restriction(
            context,
            &escalation_targets,
            CommandKind::Mute,
            Some(WARNING_MUTE_DURATION),
            Some("автоматически: три активных предупреждения"),
            true,
        )
        .await?;
        outcomes.push(format!("Автомут по порогу трёх предупреждений:\n{mutes}"));
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
    let default_permissions = if expected_action == "mute" {
        match default_chat_permissions(bot, chat_id).await {
            Ok(permissions) => Some(permissions),
            Err(_) => {
                return Ok(
                    "Снятие не выполнено: не удалось получить права чата; действий не изменено."
                        .to_string(),
                );
            }
        }
    } else {
        None
    };
    reverify_live_usernames(bot, chat_id, targets).await?;
    let claimed = manual_moderation::claim_restriction_revokes_batch(
        pool,
        batch_id,
        chat_id.0,
        &targets
            .iter()
            .map(|target| target.user_id)
            .collect::<Vec<_>>(),
        actor_id,
        expected_action,
    )
    .await?;
    let mut outcomes = Vec::new();
    for (target, prepared) in targets.iter().zip(claimed) {
        let Some(prepared) = prepared else {
            outcomes.push(format!(
                "{} — активная мера бота не найдена",
                target.user_id
            ));
            continue;
        };
        let action = prepared.action;
        match prepared.operation_status.as_str() {
            "succeeded" => {
                outcomes.push(format!("{} — ограничение уже снято", target.user_id));
                continue;
            }
            "failed" => {
                outcomes.push(format!("{} — Telegram отклонил снятие", target.user_id));
                continue;
            }
            "unknown" | "processing" => {
                outcomes.push(format!(
                    "{} — исход Telegram неизвестен; повтор запрещён",
                    target.user_id
                ));
                continue;
            }
            "pending" => {}
            status => {
                outcomes.push(format!(
                    "{} — неподдерживаемое состояние операции {status}",
                    target.user_id
                ));
                continue;
            }
        }
        if !manual_moderation::start_restriction_revoke(pool, prepared.operation_id).await? {
            outcomes.push(format!(
                "{} — снятие уже запущено другим обработчиком",
                target.user_id
            ));
            continue;
        }
        match clear_restriction(
            bot,
            chat_id,
            target.user_id,
            &action.action,
            default_permissions.clone(),
        )
        .await
        {
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
    selection: manual_moderation::WarningSelection,
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
            selection,
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
    let needs_default_permissions = actions.iter().any(|action| {
        matches!(action.action.as_str(), "mute" | "auto_mute")
            && matches!(action.status.as_str(), "applied" | "revoked")
    });
    let default_permissions = if needs_default_permissions {
        match default_chat_permissions(bot, chat_id).await {
            Ok(permissions) => Some(permissions),
            Err(_) => {
                return Ok(
                    "Отмена не выполнена: не удалось получить права чата; действий не изменено."
                        .to_string(),
                );
            }
        }
    } else {
        None
    };
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
        // Сверяем все видимые ограничения до первого изменения Telegram.
        let has_restriction_action = actions.iter().any(|candidate| {
            candidate.target_user_id == action.target_user_id
                && matches!(candidate.action.as_str(), "mute" | "ban" | "auto_mute")
                && matches!(candidate.status.as_str(), "applied" | "revoked")
        });
        if has_restriction_action {
            let local_restriction = manual_moderation::active_restriction_action(
                pool,
                chat_id.0,
                action.target_user_id,
            )
            .await?;
            if let Err(error) = validate_observed_restriction(
                observed_restriction(&member.kind),
                local_restriction.as_deref(),
                action.target_user_id,
            ) {
                return Ok(format!(
                    "Отмена не выполнена: {error}; ни одно действие не изменено."
                ));
            }
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
                let previous = if reapply {
                    None
                } else {
                    manual_moderation::undo_previous_action(pool, action.id).await?
                };
                if !manual_moderation::start_undo_operation(pool, undo_batch_id, action.id).await? {
                    outcomes.push(format!(
                        "{} — отмена уже запущена другим обработчиком",
                        action.target_user_id
                    ));
                    continue;
                }
                let result = if reapply {
                    reapply_action(bot, chat_id, &action)
                        .await
                        .map_err(|error| telegram_outcome_unknown(&error))
                } else {
                    restore_action(
                        bot,
                        chat_id,
                        &action,
                        previous.as_ref(),
                        default_permissions.clone(),
                    )
                    .await
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
        Some(expires_at) => request
            .until_date(safe_telegram_until_date(expires_at)?)
            .await
            .map(|_| ()),
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
        Some(expires_at) => request
            .until_date(safe_telegram_until_date(expires_at)?)
            .await
            .map(|_| ()),
        None => request.await.map(|_| ()),
    }
}

async fn clear_restriction(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    user_id: i64,
    action: &str,
    default_permissions: Option<ChatPermissions>,
) -> Result<(), teloxide::RequestError> {
    if action == "ban" {
        bot.unban_chat_member(chat_id, UserId(user_id as u64))
            .only_if_banned(true)
            .await
            .map(|_| ())
    } else {
        let permissions = default_permissions.ok_or_else(|| {
            teloxide::RequestError::Io(
                std::io::Error::other("chat permissions were not preflighted").into(),
            )
        })?;
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
    default_permissions: Option<ChatPermissions>,
) -> Result<(), bool> {
    let Some(previous) = previous else {
        return clear_restriction(
            bot,
            chat_id,
            current.target_user_id,
            &current.action,
            default_permissions,
        )
        .await
        .map_err(|error| telegram_outcome_unknown(&error));
    };
    if current.action == "ban" && previous.action != "ban" {
        bot.unban_chat_member(chat_id, UserId(current.target_user_id as u64))
            .only_if_banned(true)
            .await
            .map_err(|error| telegram_outcome_unknown(&error))?;
    }
    if previous.action == "ban" {
        let request = bot.ban_chat_member(chat_id, UserId(current.target_user_id as u64));
        match previous.expires_at {
            Some(expires_at) => request
                .until_date(safe_telegram_until_date(expires_at).map_err(|_| true)?)
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
        .until_date(safe_telegram_until_date(previous.expires_at.ok_or(true)?).map_err(|_| true)?)
        .await
        .map(|_| ())
        // A failed restore after unbanning has already changed Telegram state.
        .map_err(|_| true)
    }
}

fn safe_telegram_until_date(
    expires_at: chrono::DateTime<Utc>,
) -> Result<chrono::DateTime<Utc>, teloxide::RequestError> {
    const MIN_REMAINING: ChronoDuration = ChronoDuration::seconds(45);
    if expires_at.signed_duration_since(Utc::now()) < MIN_REMAINING {
        return Err(teloxide::RequestError::Io(
            std::io::Error::other(
                "temporary Telegram restriction is too close to expiry".to_string(),
            )
            .into(),
        ));
    }
    Ok(expires_at)
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

async fn send_reply(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    text: &str,
) -> ResponseResult<()> {
    let ephemeral_enabled = state
        .config
        .chat_allows(msg.chat.id.0, |chat| chat.ephemeral_command_replies);
    let Some(audience) = service_messages::command_audience(msg, ephemeral_enabled) else {
        tracing::warn!(
            chat_id = msg.chat.id.0,
            message_id = msg.id.0,
            "suppressed manual moderation reply because the message has no eligible user sender"
        );
        return Ok(());
    };
    service_messages::send_html_reply(
        bot,
        msg.chat.id,
        msg.id,
        Html::text(text).into_string(),
        audience,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, body::Bytes, extract::State, http::Uri, routing::post};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct MemberStub {
        calls: Mutex<Vec<String>>,
        replies: Mutex<Vec<Value>>,
        effects: Mutex<Vec<(String, i64)>>,
        username: Mutex<Option<String>>,
        api_error: Mutex<bool>,
    }

    async fn member_api(State(stub): State<Arc<MemberStub>>, uri: Uri, body: Bytes) -> Json<Value> {
        let method = uri
            .path()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_lowercase();
        stub.calls.lock().unwrap().push(method.clone());
        if method == "getme" {
            return Json(
                json!({"ok":true,"result":{"id":9001,"is_bot":true,"first_name":"Fake","username":"fixture_bot",
                    "can_join_groups":true,"can_read_all_group_messages":false,
                    "supports_inline_queries":false,"has_main_web_app":false}}),
            );
        }
        let payload: Value = serde_json::from_slice(&body).unwrap();
        if method == "sendmessage" {
            stub.replies.lock().unwrap().push(payload["text"].clone());
            return Json(
                json!({"ok":true,"result":{"message_id": 700,"date":1700000000,
                "chat":{"id":payload["chat_id"],"type":"supergroup","title":"Fixture"},
                "text":payload["text"]}}),
            );
        }
        if matches!(method.as_str(), "banchatsenderchat" | "unbanchatsenderchat") {
            assert!(payload.get("user_id").is_none());
            stub.effects
                .lock()
                .unwrap()
                .push((method, payload["sender_chat_id"].as_i64().unwrap()));
            if *stub.api_error.lock().unwrap() {
                return Json(
                    json!({"ok":false,"error_code":400,"description":"synthetic ambiguous effect"}),
                );
            }
            return Json(json!({"ok":true,"result":true}));
        }
        let user_id = payload["user_id"].as_i64().expect("user id");
        if matches!(method.as_str(), "banchatmember" | "restrictchatmember") {
            stub.effects.lock().unwrap().push((method, user_id));
            return Json(json!({"ok":true,"result":true}));
        }
        if matches!(user_id, 9001 | 9002) {
            return Json(
                json!({"ok":true,"result":{"status":"creator","is_anonymous":false,
                "user":{"id":user_id,"is_bot":user_id==9001,"first_name":"Fixture"}}}),
            );
        }
        if *stub.api_error.lock().unwrap() {
            return Json(json!({"ok": false, "error_code": 400, "description": "fixture failure"}));
        }
        assert_eq!(method, "getchatmember");
        let mut user = json!({"id": user_id, "is_bot": false, "first_name": "Fixture"});
        if let Some(username) = stub.username.lock().unwrap().clone() {
            user["username"] = json!(username);
        }
        Json(json!({"ok": true, "result": {"status": "member", "user": user}}))
    }

    async fn fake_member_bot(stub: Arc<MemberStub>) -> teloxide::adaptors::DefaultParseMode<Bot> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new().fallback(post(member_api)).with_state(stub);
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Bot::new("fixture-token")
            .set_api_url(format!("http://{address}/").parse().unwrap())
            .parse_mode(teloxide::types::ParseMode::Html)
    }

    async fn assert_manual_channel_path(pool: &PgPool) {
        use crate::features::manual_moderation::sender_chat::{self, Action, Request, Reservation};
        let mut config = crate::config::test_config();
        let chat_id = -1_009_900_000_101i64;
        let sender_id = -1_009_900_000_102i64;
        let chat = config.community.chats.get_mut("main").unwrap();
        chat.id = chat_id;
        chat.ingest = true;
        chat.moderation = true;
        config.chat_registry = crate::community::ChatRegistry::new(&config.community).unwrap();
        let state = AppState::new(pool.clone(), config);
        let stub = Arc::new(MemberStub::default());
        let bot = fake_member_bot(stub.clone()).await;
        let base = (Utc::now().timestamp_micros().unsigned_abs() % 500_000_000) as i32;
        let message = |id, actor, sender, auto_forward| -> Message {
            serde_json::from_value(json!({"message_id":id,"date":1700000000,
              "chat":{"id":chat_id,"type":"supergroup","title":"Synthetic"},
              "from":{"id":actor,"is_bot":false,"first_name":"Actor"},"text":"synthetic command",
              "reply_to_message":{"message_id":1,"date":1700000000,
                "chat":{"id":chat_id,"type":"supergroup","title":"Synthetic"},
                "sender_chat":{"id":sender,"type":"channel","title":"Synthetic"},
                "is_automatic_forward":auto_forward,"text":"Synthetic channel comment"}}))
            .unwrap()
        };
        let ban = message(base, 9002, sender_id, false);
        handle(&bot, &ban, &state, CommandKind::Ban, "")
            .await
            .unwrap();
        handle(&bot, &ban, &state, CommandKind::Ban, "")
            .await
            .unwrap();
        assert_eq!(
            stub.effects.lock().unwrap().as_slice(),
            &[("banchatsenderchat".into(), sender_id)]
        );
        handle(
            &bot,
            &message(base + 1, 9002, sender_id, false),
            &state,
            CommandKind::Unban,
            "",
        )
        .await
        .unwrap();
        assert_eq!(
            stub.effects.lock().unwrap()[1],
            ("unbanchatsenderchat".into(), sender_id)
        );
        for (offset, actor, sender, auto_forward, args) in [
            (2, 9003, sender_id, false, ""),
            (3, 9002, sender_id, true, ""),
            (4, 9002, chat_id, false, ""),
            (5, 9002, sender_id, false, "1h"),
            (0, 9002, sender_id - 1, false, ""),
        ] {
            handle(
                &bot,
                &message(base + offset, actor, sender, auto_forward),
                &state,
                CommandKind::Ban,
                args,
            )
            .await
            .unwrap();
        }
        assert_eq!(stub.effects.lock().unwrap().len(), 2);
        *stub.api_error.lock().unwrap() = true;
        let uncertain = message(base + 6, 9002, sender_id, false);
        handle(&bot, &uncertain, &state, CommandKind::Ban, "")
            .await
            .unwrap();
        handle(&bot, &uncertain, &state, CommandKind::Ban, "")
            .await
            .unwrap();
        handle(
            &bot,
            &message(base + 7, 9002, sender_id, false),
            &state,
            CommandKind::Unban,
            "",
        )
        .await
        .unwrap();
        assert_eq!(stub.effects.lock().unwrap().len(), 3);
        assert!(
            stub.replies
                .lock()
                .unwrap()
                .iter()
                .any(|v| v.as_str().unwrap_or("").contains("не подтвердил"))
        );
        // Два конкурентных claims и crash сразу после durable reserve.
        let a = Request {
            chat_id,
            command_message_id: base + 8,
            sender_chat_id: sender_id - 5,
            actor_user_id: 9002,
            action: Action::Ban,
            reason: None,
        };
        let b = Request {
            command_message_id: base + 9,
            action: Action::Unban,
            ..a
        };
        let (x, y) = tokio::join!(
            sender_chat::reserve(pool, &a),
            sender_chat::reserve(pool, &b)
        );
        let results = [x.unwrap(), y.unwrap()];
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Reservation::Started(_)))
                .count(),
            1
        );
        assert_eq!(
            results.iter().filter(|r| **r == Reservation::Busy).count(),
            1
        );
        let id = results
            .iter()
            .find_map(|r| {
                if let Reservation::Started(id) = r {
                    Some(*id)
                } else {
                    None
                }
            })
            .unwrap();
        let c = Request {
            command_message_id: base + 10,
            sender_chat_id: sender_id - 6,
            ..a
        };
        let (channel, user) = tokio::join!(
            sender_chat::reserve(pool, &c),
            manual_moderation::create_batch(pool, chat_id, 9002, base + 10, "ban")
        );
        assert_ne!(
            channel.is_ok(),
            user.is_ok(),
            "one command cannot reserve both target types"
        );
        assert!(
            resolve_target_requests(
                pool,
                chat_id,
                &[crate::telegram::identity::CHANNEL_SENDER_PROXY_ID.to_string()],
                None
            )
            .await
            .is_err()
        );
        sender_chat::finish(pool, id, false).await.unwrap();
        assert!(matches!(
            sender_chat::reserve(pool, &a).await.unwrap(),
            Reservation::Uncertain | Reservation::Busy
        ));
        let readable: bool = sqlx::query_scalar(
            "select has_table_privilege('nedobot_mcp_ro','manual_sender_chat_operations','SELECT')",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert!(!readable);
        sqlx::query("delete from manual_moderation_batches where chat_id=$1")
            .bind(chat_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("delete from manual_sender_chat_operations where chat_id=$1")
            .bind(chat_id)
            .execute(pool)
            .await
            .unwrap();
    }

    fn parsed_ban(username: &str) -> ParsedCommand {
        parse_command(CommandKind::Ban, username).expect("synthetic ban command")
    }

    #[tokio::test]
    #[ignore = "run against the disposable local test database"]
    async fn username_sanctions_require_live_ownership_and_resume_preserves_provenance() {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must point to the disposable test database");
        let pool = PgPool::connect(&database_url).await.unwrap();
        crate::db::migrate(&pool).await.unwrap();
        assert_manual_channel_path(&pool).await;
        let suffix = (Utc::now().timestamp_micros().unsigned_abs() % 900_000_000) as i64;
        let chat_id = -1_008_970_032_163;
        let user_id = 9_320_000_000_000 + suffix;
        let username = format!("a{suffix:08x}");
        sqlx::query("insert into telegram_user_profiles(telegram_user_id, username) values($1,$2) on conflict (telegram_user_id) do update set username=excluded.username")
            .bind(user_id).bind(&username).execute(&pool).await.unwrap();
        sqlx::query("insert into telegram_chat_users(chat_id, telegram_user_id) values($1,$2) on conflict do nothing")
            .bind(chat_id).bind(user_id).execute(&pool).await.unwrap();
        sqlx::query("insert into telegram_username_aliases(username_normalized,telegram_peer_id,peer_type,source) values($1,$2,'user','profile_refresh')")
            .bind(&username).bind(user_id).execute(&pool).await.unwrap();

        let stub = Arc::new(MemberStub::default());
        *stub.username.lock().unwrap() = Some("renamed_away".to_string());
        let bot = fake_member_bot(stub.clone()).await;

        let resolved = resolve_target_requests(&pool, chat_id, &[format!("@{username}")], None)
            .await
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].user_id, user_id);
        assert_eq!(resolved[0].requested_username(), Some(username.as_str()));

        let error = checked_targets(
            &bot,
            &pool,
            ChatId(chat_id),
            CommandKind::Ban,
            &resolved,
            None,
        )
        .await
        .expect_err("a reassigned username must fail closed");
        assert!(error.to_string().contains("больше не подтверждается"));
        assert_eq!(
            stub.calls.lock().unwrap().as_slice(),
            ["getchatmember"],
            "the mismatch must stop before any sanction API call"
        );

        // Reply and explicit numeric ID keep their normal policy.
        let reply = resolve_target_requests(&pool, chat_id, &[], Some(user_id))
            .await
            .unwrap();
        assert_eq!(
            reply[0].resolution_source,
            crate::features::manual_moderation::types::TargetResolutionSource::Reply
        );
        assert!(
            checked_targets(&bot, &pool, ChatId(chat_id), CommandKind::Ban, &reply, None)
                .await
                .is_ok()
        );
        let numeric = vec![Target::numeric(user_id)];
        assert!(
            checked_targets(
                &bot,
                &pool,
                ChatId(chat_id),
                CommandKind::Ban,
                &numeric,
                None
            )
            .await
            .is_ok()
        );

        // Resume rebuilds the same live-ownership obligation from the snapshot.
        let parsed = parsed_ban(&format!("@{username}"));
        let snapshot = BatchRequestSnapshot::from_parsed(&parsed, resolved.clone());
        let resumed = snapshot.resume_targets().unwrap();
        assert!(
            checked_targets(
                &bot,
                &pool,
                ChatId(chat_id),
                CommandKind::Ban,
                &resumed,
                None
            )
            .await
            .is_err()
        );
        let legacy = BatchRequestSnapshot {
            kind: CommandKind::Ban,
            target_user_ids: vec![user_id],
            targets: Vec::new(),
            duration_seconds: None,
            reason: None,
            warn_id: None,
            warn_all: false,
            limit: 20,
        };
        assert!(
            legacy.resume_targets().is_err(),
            "legacy snapshots fail closed"
        );

        // Missing username and API errors also fail closed.
        *stub.username.lock().unwrap() = None;
        assert!(
            checked_targets(
                &bot,
                &pool,
                ChatId(chat_id),
                CommandKind::Ban,
                &resolved,
                None
            )
            .await
            .is_err()
        );
        *stub.api_error.lock().unwrap() = true;
        assert!(
            checked_targets(
                &bot,
                &pool,
                ChatId(chat_id),
                CommandKind::Ban,
                &resolved,
                None
            )
            .await
            .is_err()
        );
        assert!(
            stub.calls
                .lock()
                .unwrap()
                .iter()
                .all(|method| method == "getchatmember"),
            "no sanction API method may be reached while ownership is unconfirmed"
        );

        let state = AppState::new(pool.clone(), crate::config::test_config());
        let message = |message_id, reply| {
            let chat = json!({"id":chat_id,"type":"supergroup","title":"Fixture"});
            let mut msg = json!({"message_id":message_id,"date":1700000000,"chat":chat,
                "from":{"id":9002,"is_bot":false,"first_name":"Fixture"},"text":"synthetic command"});
            if reply {
                msg["reply_to_message"] = json!({"message_id":44,"date":1700000000,"chat":chat,
                    "from":{"id":user_id,"is_bot":false,"first_name":"Fixture"},"text":"synthetic reply"});
            }
            serde_json::from_value::<Message>(msg).unwrap()
        };
        let first_message_id = (suffix as i32) * 2;
        let target = format!("@{}", username.to_ascii_uppercase());
        *stub.api_error.lock().unwrap() = false;
        *stub.username.lock().unwrap() = Some("renamed_away".into());
        for (index, kind) in [CommandKind::Ban, CommandKind::Mute, CommandKind::Warn]
            .into_iter()
            .enumerate()
        {
            let msg = message(first_message_id + index as i32, false);
            handle(&bot, &msg, &state, kind, &target).await.unwrap();
            assert!(
                manual_moderation::find_batch(&pool, chat_id, msg.id.0)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        let resume_msg = message(first_message_id + 3, false);
        manual_moderation::create_batch_with_request(
            &pool,
            chat_id,
            9002,
            resume_msg.id.0,
            "ban",
            &snapshot,
        )
        .await
        .unwrap();
        handle(&bot, &resume_msg, &state, CommandKind::Ban, &target)
            .await
            .unwrap();
        assert!(stub.effects.lock().unwrap().is_empty());
        handle(
            &bot,
            &message(first_message_id + 4, true),
            &state,
            CommandKind::Mute,
            "",
        )
        .await
        .unwrap();
        handle(
            &bot,
            &message(first_message_id + 5, false),
            &state,
            CommandKind::Ban,
            &user_id.to_string(),
        )
        .await
        .unwrap();
        assert_eq!(
            stub.effects.lock().unwrap().as_slice(),
            [
                ("restrictchatmember".into(), user_id),
                ("banchatmember".into(), user_id)
            ],
            "methods={:?}, replies={:?}",
            stub.calls.lock().unwrap(),
            stub.replies.lock().unwrap(),
        );

        sqlx::query("delete from telegram_username_aliases where username_normalized=$1")
            .bind(&username)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("delete from telegram_chat_users where chat_id=$1 and telegram_user_id=$2")
            .bind(chat_id)
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("delete from telegram_user_profiles where telegram_user_id=$1")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    #[test]
    fn escalation_action_uses_fixed_five_day_expiration() {
        assert_eq!(
            WARNING_MUTE_DURATION,
            std::time::Duration::from_secs(5 * 24 * 60 * 60)
        );
    }

    #[test]
    fn unauthorized_actors_never_pass_the_manual_effect_gate() {
        assert!(!actor_rights_allow_action(false, true, true, true));
        assert!(!actor_rights_allow_action(true, false, true, true));
        assert!(!actor_rights_allow_action(true, true, false, true));
        assert!(actor_rights_allow_action(true, true, true, true));
        assert!(actor_rights_allow_action(true, false, false, false));
    }

    #[test]
    fn temporary_telegram_restrictions_keep_a_margin_above_the_api_30_second_boundary() {
        assert!(safe_telegram_until_date(Utc::now() + ChronoDuration::seconds(44)).is_err());
        assert!(safe_telegram_until_date(Utc::now() + ChronoDuration::seconds(60)).is_ok());
    }

    #[test]
    fn moderation_messages_are_deleted_only_after_applied_negative_actions() {
        let action = |action: &str, status: &str| ActionRecord {
            id: 1,
            batch_id: 1,
            chat_id: -1001,
            target_user_id: 2,
            actor_user_id: 3,
            action: action.to_string(),
            reason: None,
            status: status.to_string(),
            created_at: Utc::now(),
            expires_at: None,
            supersedes_action_id: None,
            automatic: false,
        };

        assert!(should_delete_moderation_messages(
            CommandKind::Ban,
            &[action("ban", "applied")]
        ));
        assert!(should_delete_moderation_messages(
            CommandKind::Mute,
            &[action("mute", "applied")]
        ));
        assert!(should_delete_moderation_messages(
            CommandKind::Warn,
            &[action("warn", "applied")]
        ));
        assert!(!should_delete_moderation_messages(
            CommandKind::Ban,
            &[action("ban", "failed")]
        ));
        assert!(!should_delete_moderation_messages(
            CommandKind::Unban,
            &[action("ban", "applied")]
        ));
    }

    #[test]
    fn external_telegram_restrictions_fail_closed_without_matching_local_action() {
        assert!(validate_observed_restriction(ObservedRestriction::Restricted, None, 10).is_err());
        assert!(
            validate_observed_restriction(ObservedRestriction::Restricted, Some("mute"), 10)
                .is_ok()
        );
        assert!(
            validate_observed_restriction(ObservedRestriction::Restricted, Some("auto_mute"), 10)
                .is_ok()
        );
        assert!(
            validate_observed_restriction(ObservedRestriction::Restricted, Some("ban"), 10)
                .is_err()
        );
        assert!(validate_observed_restriction(ObservedRestriction::Banned, None, 10).is_err());
        assert!(
            validate_observed_restriction(ObservedRestriction::Banned, Some("ban"), 10).is_ok()
        );
        assert!(
            validate_observed_restriction(ObservedRestriction::Banned, Some("mute"), 10).is_err()
        );
        assert!(validate_observed_restriction(ObservedRestriction::None, None, 10).is_ok());
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
