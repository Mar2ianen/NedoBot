//! Adapter ручного ban/unban канала; права проверяет вызывающий manual handler.
use crate::features::manual_moderation::{
    sender_chat::{self, Action, Request, Reservation},
    types::{CommandKind, ParsedCommand},
};
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
    let id=match reservation {
        Reservation::Succeeded=>return Ok(success(action)),
        Reservation::Uncertain|Reservation::Busy=>return Ok("Для этого канала есть операция с неподтверждённым исходом. Повтор не выполнен; проверь ограничения в Telegram и журнал операций.".into()),
        Reservation::Started(id)=>id,
    };
    let call = async {
        match action {
            Action::Ban => bot.ban_chat_sender_chat(msg.chat.id, sender.id).await,
            Action::Unban => bot.unban_chat_sender_chat(msg.chat.id, sender.id).await,
        }
    };
    let acknowledged = matches!(
        tokio::time::timeout(std::time::Duration::from_secs(15), call).await,
        Ok(Ok(_))
    );
    sender_chat::finish(pool, id, acknowledged).await?;
    if acknowledged {
        Ok(success(action))
    } else {
        Ok("Telegram не подтвердил результат операции с каналом. Проверь ограничения вручную; автоматического повтора не будет.".into())
    }
}
fn success(action: Action) -> String {
    match action {
        Action::Ban => "Отправка сообщений от этого канала в чате заблокирована.",
        Action::Unban => "Блокировка отправки сообщений от этого канала в чате снята.",
    }
    .into()
}
