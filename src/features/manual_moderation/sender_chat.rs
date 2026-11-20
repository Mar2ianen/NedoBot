//! Ручные channel sanctions: отдельный журнал, без account spam labels и retry.
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Ban,
    Unban,
}
impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ban => "ban",
            Self::Unban => "unban",
        }
    }
}
#[derive(Debug)]
pub struct Request<'a> {
    pub chat_id: i64,
    pub command_message_id: i32,
    pub sender_chat_id: i64,
    pub actor_user_id: i64,
    pub action: Action,
    pub reason: Option<&'a str>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Reservation {
    Started(i64),
    Succeeded,
    Uncertain,
    Busy,
}

pub async fn reserve(pool: &PgPool, r: &Request<'_>) -> anyhow::Result<Reservation> {
    anyhow::ensure!(
        r.sender_chat_id < 0 && r.sender_chat_id != r.chat_id && r.actor_user_id > 0,
        "invalid channel target"
    );
    let mut tx = pool.begin().await?;
    lock_command(&mut tx, r.chat_id, r.command_message_id).await?;
    let user_command:bool=sqlx::query_scalar("select exists(select 1 from manual_moderation_batches where chat_id=$1 and source_message_id=$2)")
        .bind(r.chat_id).bind(r.command_message_id).fetch_one(&mut *tx).await?;
    anyhow::ensure!(!user_command, "command already registered for a user");

    // Одновременные противоположные команды одной цели не выполняются параллельно.
    sqlx::query("select pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "manual-sender-chat:{}:{}",
            r.chat_id, r.sender_chat_id
        ))
        .execute(&mut *tx)
        .await?;
    let old:Option<(i64,i64,String,String,Option<String>)>=sqlx::query_as("select sender_chat_id,actor_user_id,action,status,reason from manual_sender_chat_operations where chat_id=$1 and command_message_id=$2")
        .bind(r.chat_id).bind(r.command_message_id).fetch_optional(&mut *tx).await?;
    if let Some((target, actor, action, status, reason)) = old {
        anyhow::ensure!(
            target == r.sender_chat_id
                && actor == r.actor_user_id
                && action == r.action.as_str()
                && reason.as_deref() == r.reason,
            "channel command changed; send a new command"
        );
        return match status.as_str() {
            "succeeded" => Ok(Reservation::Succeeded),
            "started" | "unknown" => Ok(Reservation::Uncertain),
            _ => anyhow::bail!("invalid channel operation status"),
        };
    }
    let blocked:bool=sqlx::query_scalar("select exists(select 1 from manual_sender_chat_operations where chat_id=$1 and sender_chat_id=$2 and status in ('started','unknown'))")
        .bind(r.chat_id).bind(r.sender_chat_id).fetch_one(&mut *tx).await?;
    if blocked {
        return Ok(Reservation::Busy);
    }
    let id:i64=sqlx::query_scalar("insert into manual_sender_chat_operations(chat_id,command_message_id,sender_chat_id,actor_user_id,action,reason,status) values($1,$2,$3,$4,$5,$6,'started') returning id")
        .bind(r.chat_id).bind(r.command_message_id).bind(r.sender_chat_id).bind(r.actor_user_id).bind(r.action.as_str()).bind(r.reason).fetch_one(&mut *tx).await?;
    // Сохраняем ДО внешнего эффекта: crash/timeout не даёт разрешения на повтор.
    tx.commit().await?;
    Ok(Reservation::Started(id))
}

pub async fn finish(pool: &PgPool, id: i64, acknowledged: bool) -> anyhow::Result<()> {
    let rows=sqlx::query("update manual_sender_chat_operations set status=$2,completed_at=now() where id=$1 and status='started'")
        .bind(id).bind(if acknowledged {"succeeded"}else{"unknown"}).execute(pool).await?.rows_affected();
    anyhow::ensure!(rows == 1, "channel operation completion lost ownership");
    Ok(())
}

/// Не позволяет edited update переключить уже зарегистрированную цель на user path.
pub async fn command_exists(pool: &PgPool, chat: i64, message: i32) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar("select exists(select 1 from manual_sender_chat_operations where chat_id=$1 and command_message_id=$2)").bind(chat).bind(message).fetch_one(pool).await?)
}

// Общий lock для двух журналов не позволяет edited command сменить тип цели в гонке.
pub(super) async fn lock_command(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    chat: i64,
    message: i32,
) -> anyhow::Result<()> {
    sqlx::query("select pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("manual-command:{chat}:{message}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}
