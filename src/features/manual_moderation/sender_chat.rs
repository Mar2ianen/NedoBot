//! Ручные channel sanctions: отдельный журнал, без account spam labels и retry.
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// После этого возраста `started` уже не может принадлежать живому обработчику:
/// сам Telegram-вызов ограничен 15 секундами. Подтверждать более свежую
/// операцию опасно — reconciliation мог бы разойтись с ещё идущим запросом.
pub const STARTED_RECONCILE_GRACE_SECONDS: i64 = 120;

/// Bounded Telegram-выдача: 8 строк с коротким preview причины помещаются
/// в одно сообщение даже после HTML-экранирования.
pub const PENDING_PAGE_SIZE: i64 = 8;

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

/// Административное подтверждение фактического состояния в Telegram.
/// Это не повтор API-вызова, а фиксация уже проверенного вручную исхода.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Applied,
    NotApplied,
}
impl Resolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::NotApplied => "not_applied",
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "applied" => Ok(Self::Applied),
            "not_applied" => Ok(Self::NotApplied),
            _ => anyhow::bail!("подтверди состояние словом applied или not_applied"),
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
    /// Команда уже подтверждена как невыполненная; повторять её нельзя.
    Failed,
    /// Исход этой же команды ещё не подтверждён.
    Uncertain(i64),
    /// У канала есть другая операция с неподтверждённым исходом.
    Busy(i64),
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
    let old:Option<(i64,i64,i64,String,String,Option<String>)>=sqlx::query_as("select id,sender_chat_id,actor_user_id,action,status,reason from manual_sender_chat_operations where chat_id=$1 and command_message_id=$2")
        .bind(r.chat_id).bind(r.command_message_id).fetch_optional(&mut *tx).await?;
    if let Some((id, target, actor, action, status, reason)) = old {
        anyhow::ensure!(
            target == r.sender_chat_id
                && actor == r.actor_user_id
                && action == r.action.as_str()
                && reason.as_deref() == r.reason,
            "channel command changed; send a new command"
        );
        return match status.as_str() {
            "succeeded" => Ok(Reservation::Succeeded),
            "failed" => Ok(Reservation::Failed),
            "started" | "unknown" => Ok(Reservation::Uncertain(id)),
            _ => anyhow::bail!("invalid channel operation status"),
        };
    }
    let blocked:Option<i64>=sqlx::query_scalar("select id from manual_sender_chat_operations where chat_id=$1 and sender_chat_id=$2 and status in ('started','unknown') order by id desc limit 1")
        .bind(r.chat_id).bind(r.sender_chat_id).fetch_optional(&mut *tx).await?;
    if let Some(id) = blocked {
        return Ok(Reservation::Busy(id));
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

#[derive(Debug, sqlx::FromRow)]
pub struct PendingOperation {
    pub id: i64,
    pub sender_chat_id: i64,
    pub command_message_id: i32,
    pub actor_user_id: i64,
    pub action: String,
    pub reason: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct PendingPage {
    pub operations: Vec<PendingOperation>,
    pub total: i64,
    pub page: u32,
}

/// Операции, которые блокируют новые команды этого канала. Выдача ограничена
/// одной страницей, чтобы `/channelops` не превышал лимит Telegram.
pub async fn pending(pool: &PgPool, chat_id: i64, page: u32) -> anyhow::Result<PendingPage> {
    anyhow::ensure!(page > 0, "invalid channel operations page");
    let total: i64 = sqlx::query_scalar(
        "select count(*) from manual_sender_chat_operations
         where chat_id = $1 and status in ('started', 'unknown')",
    )
    .bind(chat_id)
    .fetch_one(pool)
    .await?;
    let offset = i64::from(page - 1) * PENDING_PAGE_SIZE;
    let operations = sqlx::query_as::<_, PendingOperation>(
        r#"select id, sender_chat_id, command_message_id, actor_user_id, action, reason,
                  status, created_at
           from manual_sender_chat_operations
           where chat_id = $1 and status in ('started', 'unknown')
           order by id
           limit $2 offset $3"#,
    )
    .bind(chat_id)
    .bind(PENDING_PAGE_SIZE)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(PendingPage {
        operations,
        total,
        page,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReconcileOutcome {
    Resolved {
        previous_status: String,
        sender_chat_id: i64,
        action: String,
    },
    NotFound,
    AlreadyResolved {
        status: String,
    },
    TooRecent {
        retry_after_seconds: i64,
    },
}

/// Фиксирует проверенный администратором исход. Telegram не вызывается:
/// решение принимает человек, бот только снимает блокировку журнала.
pub async fn reconcile(
    pool: &PgPool,
    chat_id: i64,
    operation_id: i64,
    actor_user_id: i64,
    resolution: Resolution,
    note: Option<&str>,
) -> anyhow::Result<ReconcileOutcome> {
    anyhow::ensure!(
        operation_id > 0 && actor_user_id > 0,
        "invalid reconciliation request"
    );
    let mut tx = pool.begin().await?;
    let Some((sender_chat_id,)): Option<(i64,)> = sqlx::query_as(
        "select sender_chat_id from manual_sender_chat_operations where id=$1 and chat_id=$2",
    )
    .bind(operation_id)
    .bind(chat_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        tx.rollback().await?;
        return Ok(ReconcileOutcome::NotFound);
    };
    // Тот же per-target lock, что и в reserve: новая команда канала не
    // проскочит между проверкой и фиксацией подтверждения.
    sqlx::query("select pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("manual-sender-chat:{chat_id}:{sender_chat_id}"))
        .execute(&mut *tx)
        .await?;
    let row: Option<(i64, String, String, DateTime<Utc>)> = sqlx::query_as(
        r#"select sender_chat_id, action, status, created_at
           from manual_sender_chat_operations
           where id = $1 and chat_id = $2
           for update"#,
    )
    .bind(operation_id)
    .bind(chat_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((sender_chat_id, action, status, created_at)) = row else {
        tx.rollback().await?;
        return Ok(ReconcileOutcome::NotFound);
    };
    if !matches!(status.as_str(), "started" | "unknown") {
        tx.rollback().await?;
        return Ok(ReconcileOutcome::AlreadyResolved { status });
    }
    if status == "started" {
        let age = Utc::now()
            .signed_duration_since(created_at)
            .num_seconds()
            .max(0);
        if age < STARTED_RECONCILE_GRACE_SECONDS {
            tx.rollback().await?;
            return Ok(ReconcileOutcome::TooRecent {
                retry_after_seconds: STARTED_RECONCILE_GRACE_SECONDS - age,
            });
        }
    }
    let new_status = match resolution {
        Resolution::Applied => "succeeded",
        Resolution::NotApplied => "failed",
    };
    let changed = sqlx::query(
        r#"update manual_sender_chat_operations
           set status = $3, completed_at = now(), reconciled_at = now(),
               reconciled_by = $4, resolution = $5, reconciliation_note = $6
           where id = $1 and chat_id = $2 and status = $7"#,
    )
    .bind(operation_id)
    .bind(chat_id)
    .bind(new_status)
    .bind(actor_user_id)
    .bind(resolution.as_str())
    .bind(note)
    .bind(&status)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    anyhow::ensure!(changed == 1, "channel reconciliation lost ownership");
    tx.commit().await?;
    Ok(ReconcileOutcome::Resolved {
        previous_status: status,
        sender_chat_id,
        action,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_parser_accepts_only_explicit_states() {
        assert_eq!(Resolution::parse("applied").unwrap(), Resolution::Applied);
        assert_eq!(
            Resolution::parse("not_applied").unwrap(),
            Resolution::NotApplied
        );
        assert!(Resolution::parse("maybe").is_err());
        assert!(Resolution::parse("").is_err());
    }

    #[test]
    fn started_operations_have_a_reconciliation_grace_period() {
        const {
            assert!(STARTED_RECONCILE_GRACE_SECONDS > 15);
        }
    }
}
