use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use std::time::Duration;

use super::types::{BatchRequestSnapshot, WARNING_TTL, should_escalate_warning};

const ACTION_LEASE: ChronoDuration = ChronoDuration::minutes(2);

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ActionRecord {
    pub id: i64,
    pub batch_id: i64,
    pub chat_id: i64,
    pub target_user_id: i64,
    pub actor_user_id: i64,
    pub action: String,
    pub reason: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub supersedes_action_id: Option<i64>,
    pub automatic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndoClaim {
    WarningRevoked,
    WarningRestored,
    RestrictionClaimed,
    RestrictionReapplyClaimed,
    Conflict,
    Expired,
}

#[derive(Debug, Clone)]
pub struct WarningResult {
    pub action_id: i64,
    pub active_count: i64,
    pub should_escalate: bool,
}

#[derive(Debug, Clone)]
pub struct BatchCreation {
    pub id: i64,
    pub actor_user_id: i64,
    pub command: String,
    pub status: String,
    pub request: Option<BatchRequestSnapshot>,
    pub result_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchClaim {
    Claimed,
    Busy,
    Finished(Option<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningSelection {
    Latest,
    All,
    Id(i64),
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PreparedAction {
    pub id: i64,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct PreparedRestrictionRevoke {
    pub operation_id: i64,
    pub operation_status: String,
    pub action: ActionRecord,
}

#[derive(Debug, Clone, Copy)]
pub struct ActionPreparation<'a> {
    pub batch_id: i64,
    pub chat_id: i64,
    pub target_user_id: i64,
    pub actor_user_id: i64,
    pub action: &'a str,
    pub reason: Option<&'a str>,
    pub expires_at: Option<DateTime<Utc>>,
    pub automatic: bool,
}

#[allow(dead_code)] // Kept for existing integrations and moderation tooling.
pub async fn create_batch(
    pool: &PgPool,
    chat_id: i64,
    actor_user_id: i64,
    source_message_id: i32,
    command: &str,
) -> anyhow::Result<BatchCreation> {
    create_batch_inner(
        pool,
        chat_id,
        actor_user_id,
        source_message_id,
        command,
        &Value::Object(Default::default()),
    )
    .await
}

pub async fn create_batch_with_request(
    pool: &PgPool,
    chat_id: i64,
    actor_user_id: i64,
    source_message_id: i32,
    command: &str,
    request: &BatchRequestSnapshot,
) -> anyhow::Result<BatchCreation> {
    create_batch_inner(
        pool,
        chat_id,
        actor_user_id,
        source_message_id,
        command,
        &serde_json::to_value(request)?,
    )
    .await
}

pub async fn find_batch(
    pool: &PgPool,
    chat_id: i64,
    source_message_id: i32,
) -> anyhow::Result<Option<BatchCreation>> {
    let row = sqlx::query_as::<_, BatchRow>(
        r#"select id, actor_user_id, command, status, request_json, result_text
           from manual_moderation_batches
           where chat_id = $1 and source_message_id = $2"#,
    )
    .bind(chat_id)
    .bind(source_message_id)
    .fetch_optional(pool)
    .await?;
    row.map(BatchRow::into_creation).transpose()
}

pub async fn claim_batch(pool: &PgPool, batch_id: i64) -> anyhow::Result<BatchClaim> {
    let mut tx = pool.begin().await?;
    let row: (String, Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(
        r#"select status, result_text, processing_lease_expires_at
           from manual_moderation_batches where id = $1 for update"#,
    )
    .bind(batch_id)
    .fetch_one(&mut *tx)
    .await?;
    if row.0 == "completed" {
        tx.commit().await?;
        return Ok(BatchClaim::Finished(row.1));
    }
    if row.0 == "unknown" {
        let resumable_work: bool = sqlx::query_scalar(
            r#"select exists (
                 select 1 from manual_moderation_actions where batch_id = $1 and status = 'pending'
                 union all
                 select 1 from manual_moderation_events e
                 join manual_moderation_actions a on a.id = e.action_id
                 where e.batch_id = $1 and a.status = 'pending'
                 union all
                 select 1 from manual_moderation_operations
                 where batch_id = $1 and status = 'pending'
               )"#,
        )
        .bind(batch_id)
        .fetch_one(&mut *tx)
        .await?;
        if !resumable_work {
            tx.commit().await?;
            return Ok(BatchClaim::Finished(row.1));
        }
    }
    if row.2.is_some_and(|expires| expires > Utc::now()) {
        tx.commit().await?;
        return Ok(BatchClaim::Busy);
    }
    sqlx::query(
        r#"update manual_moderation_batches
           set status = 'running', processing_lease_expires_at = now() + $2::interval,
               updated_at = now()
           where id = $1"#,
    )
    .bind(batch_id)
    .bind(format!("{} seconds", ACTION_LEASE.num_seconds()))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(BatchClaim::Claimed)
}

pub async fn recover_expired_batch_actions(pool: &PgPool, batch_id: i64) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"update manual_moderation_actions a
           set status = 'unknown', processing_lease_expires_at = null
           where a.status = 'processing' and a.processing_lease_expires_at <= now()
             and (
               a.batch_id = $1
             or exists (
               select 1 from manual_moderation_events e
               where e.batch_id = $1 and e.action_id = a.id
             )
             )
             and not exists (
               select 1 from manual_moderation_operations o
               where o.action_id = a.id and o.status = 'processing'
             )"#,
    )
    .bind(batch_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"with expired as (
               update manual_moderation_operations
               set status = 'unknown', lease_expires_at = null,
                   details = 'operation lease expired; Telegram outcome requires reconciliation',
                   updated_at = now()
               where status = 'processing'
                 and (batch_id = $1 or exists (
                     select 1 from manual_moderation_events e
                     where e.batch_id = $1 and e.action_id = manual_moderation_operations.action_id
                 ))
                 and (lease_expires_at <= now() or exists (
                     select 1 from manual_moderation_actions a
                     where a.id = manual_moderation_operations.action_id
                       and a.status = 'processing'
                       and a.processing_lease_expires_at <= now()
                 ))
               returning id, batch_id, action_id, chat_id, target_user_id, actor_user_id
           ), changed_actions as (
               update manual_moderation_actions a
               set status = 'unknown', processing_lease_expires_at = null
               from expired o
               where a.id = o.action_id
                 and a.status in ('pending', 'processing', 'applied', 'revoked')
               returning a.id, o.batch_id, o.chat_id, o.target_user_id, o.actor_user_id
           )
           insert into manual_moderation_events
               (batch_id, action_id, chat_id, target_user_id, actor_user_id, event, details)
           select batch_id, id, chat_id, target_user_id, actor_user_id, 'unknown',
                  'operation lease expired; Telegram outcome requires reconciliation'
           from changed_actions"#,
    )
    .bind(batch_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn batch_has_uncertain_target(
    pool: &PgPool,
    batch_id: i64,
    target_user_id: i64,
) -> anyhow::Result<bool> {
    sqlx::query_scalar(
        r#"select exists (
               select 1 from manual_moderation_actions a
               where a.target_user_id = $2 and a.status in ('processing', 'unknown')
                 and (a.batch_id = $1 or exists (
                     select 1 from manual_moderation_events e
                     where e.batch_id = $1 and e.action_id = a.id
                 ))
               union all
               select 1 from manual_moderation_operations
               where batch_id = $1 and target_user_id = $2
                 and status in ('processing', 'unknown')
           )"#,
    )
    .bind(batch_id)
    .bind(target_user_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

pub async fn finish_batch(
    pool: &PgPool,
    batch_id: i64,
    result_text: &str,
) -> anyhow::Result<String> {
    let mut tx = pool.begin().await?;
    let (has_pending, has_unknown): (bool, bool) = sqlx::query_as(
        r#"with relevant_actions as (
               select id, status from manual_moderation_actions where batch_id = $1
               union
               select a.id, a.status
               from manual_moderation_events e
               join manual_moderation_actions a on a.id = e.action_id
               where e.batch_id = $1
           )
           select
               coalesce((select bool_or(status = 'pending') from relevant_actions), false)
                 or coalesce((select bool_or(status = 'pending') from manual_moderation_operations where batch_id = $1), false),
               coalesce((select bool_or(status in ('unknown', 'processing')) from relevant_actions), false)
                 or coalesce((select bool_or(status in ('unknown', 'processing')) from manual_moderation_operations where batch_id = $1), false)"#,
    )
    .bind(batch_id)
    .fetch_one(&mut *tx)
    .await?;
    let status = if has_pending {
        "prepared"
    } else if has_unknown {
        "unknown"
    } else {
        "completed"
    };
    sqlx::query(
        r#"update manual_moderation_batches
           set status = $2, result_text = $3, processing_lease_expires_at = null,
               updated_at = now()
           where id = $1"#,
    )
    .bind(batch_id)
    .bind(status)
    .bind(result_text)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(status.to_string())
}

#[derive(sqlx::FromRow)]
struct BatchRow {
    id: i64,
    actor_user_id: i64,
    command: String,
    status: String,
    request_json: Value,
    result_text: Option<String>,
}

impl BatchRow {
    fn into_creation(self) -> anyhow::Result<BatchCreation> {
        let request = if self.request_json == Value::Object(Default::default()) {
            None
        } else {
            Some(serde_json::from_value(self.request_json)?)
        };
        Ok(BatchCreation {
            id: self.id,
            actor_user_id: self.actor_user_id,
            command: self.command,
            status: self.status,
            request,
            result_text: self.result_text,
        })
    }
}

async fn create_batch_inner(
    pool: &PgPool,
    chat_id: i64,
    actor_user_id: i64,
    source_message_id: i32,
    command: &str,
    request_json: &Value,
) -> anyhow::Result<BatchCreation> {
    let inserted: Option<(i64,)> = sqlx::query_as(
        r#"insert into manual_moderation_batches
               (chat_id, actor_user_id, source_message_id, command, request_json, status)
           values ($1, $2, $3, $4, $5, 'accepted')
           on conflict (chat_id, source_message_id) do nothing
           returning id"#,
    )
    .bind(chat_id)
    .bind(actor_user_id)
    .bind(source_message_id)
    .bind(command)
    .bind(request_json)
    .fetch_optional(pool)
    .await?;
    let batch = if let Some((id,)) = inserted {
        sqlx::query_as::<_, BatchRow>(
            r#"select id, actor_user_id, command, status, request_json, result_text
               from manual_moderation_batches where id = $1"#,
        )
        .bind(id)
        .fetch_one(pool)
        .await?
        .into_creation()?
    } else {
        find_batch(pool, chat_id, source_message_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("idempotent moderation batch disappeared"))?
    };
    if batch.actor_user_id != actor_user_id {
        anyhow::bail!("эта команда уже зарегистрирована от другого администратора");
    }
    Ok(batch)
}

#[allow(dead_code)] // Single-action API remains available to external feature consumers.
pub async fn prepare_action(
    pool: &PgPool,
    request: ActionPreparation<'_>,
) -> anyhow::Result<Option<i64>> {
    let mut tx = pool.begin().await?;
    lock_target(&mut tx, request.chat_id, request.target_user_id).await?;
    let action = prepare_action_locked(&mut tx, request, "processing", false).await?;
    tx.commit().await?;
    Ok(action.map(|action| action.id))
}

pub async fn prepare_actions_batch(
    pool: &PgPool,
    requests: &[ActionPreparation<'_>],
) -> anyhow::Result<Vec<Option<PreparedAction>>> {
    if requests.is_empty() {
        return Ok(Vec::new());
    }
    let batch_id = requests[0].batch_id;
    let chat_id = requests[0].chat_id;
    if requests
        .iter()
        .any(|request| request.batch_id != batch_id || request.chat_id != chat_id)
    {
        anyhow::bail!("batch preflight must contain one chat and one batch");
    }
    let mut target_ids = requests
        .iter()
        .map(|request| request.target_user_id)
        .collect::<Vec<_>>();
    target_ids.sort_unstable();
    target_ids.dedup();
    if target_ids.len() != requests.len() {
        anyhow::bail!("batch preflight contains duplicate targets");
    }

    let mut tx = pool.begin().await?;
    for target_user_id in target_ids {
        lock_target(&mut tx, chat_id, target_user_id).await?;
    }
    let mut prepared = Vec::with_capacity(requests.len());
    for request in requests {
        prepared.push(prepare_action_locked(&mut tx, *request, "pending", true).await?);
    }
    tx.commit().await?;
    Ok(prepared)
}

pub async fn start_prepared_action(pool: &PgPool, action_id: i64) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let action = load_action(&mut tx, action_id).await?;
    lock_target(&mut tx, action.chat_id, action.target_user_id).await?;
    let changed = sqlx::query(
        r#"update manual_moderation_operations
           set status = 'processing', lease_expires_at = now() + $2::interval,
               updated_at = now()
           where action_id = $1 and operation_kind = 'apply' and status = 'pending'"#,
    )
    .bind(action_id)
    .bind(format!("{} seconds", ACTION_LEASE.num_seconds()))
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 1 {
        let action_changed = sqlx::query(
            r#"update manual_moderation_actions
               set status = 'processing', processing_lease_expires_at = now() + $2::interval
               where id = $1 and status = 'pending'"#,
        )
        .bind(action_id)
        .bind(format!("{} seconds", ACTION_LEASE.num_seconds()))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if action_changed != 1 {
            anyhow::bail!("prepared action left pending state before Telegram request");
        }
        sqlx::query(
            r#"update manual_moderation_batches
               set processing_lease_expires_at = now() + interval '5 minutes', updated_at = now()
               where id = $1"#,
        )
        .bind(action.batch_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(changed == 1)
}

async fn prepare_action_locked(
    tx: &mut Transaction<'_, Postgres>,
    request: ActionPreparation<'_>,
    initial_status: &str,
    idempotent: bool,
) -> anyhow::Result<Option<PreparedAction>> {
    let ActionPreparation {
        batch_id,
        chat_id,
        target_user_id,
        actor_user_id,
        action,
        reason,
        expires_at,
        automatic,
    } = request;
    expire_target_actions(tx, chat_id, target_user_id).await?;

    if idempotent {
        let existing = sqlx::query_as::<_, PreparedAction>(
            r#"select id, status, expires_at
               from manual_moderation_actions
               where batch_id = $1 and target_user_id = $2 and action = $3
               order by id desc limit 1"#,
        )
        .bind(batch_id)
        .bind(target_user_id)
        .bind(action)
        .fetch_optional(&mut **tx)
        .await?;
        if existing.is_some() {
            return Ok(existing);
        }
    }

    let processing: bool = sqlx::query_scalar(
        r#"select exists (
             select 1 from manual_moderation_actions
             where chat_id = $1 and target_user_id = $2 and status in ('pending', 'processing')
             union all
             select 1 from manual_moderation_operations
             where chat_id = $1 and target_user_id = $2 and status in ('pending', 'processing')
           )"#,
    )
    .bind(chat_id)
    .bind(target_user_id)
    .fetch_one(&mut **tx)
    .await?;
    if processing {
        anyhow::bail!("у пользователя уже выполняется другая команда модерации");
    }

    let unknown_restriction: bool = sqlx::query_scalar(
        r#"select exists (
             select 1 from manual_moderation_actions
             where chat_id = $1 and target_user_id = $2 and status = 'unknown'
               and action in ('mute', 'ban', 'auto_mute')
               and (expires_at is null or expires_at > now())
             union all
             select 1 from manual_moderation_operations o
             join manual_moderation_actions a on a.id = o.action_id
             where o.chat_id = $1 and o.target_user_id = $2 and o.status = 'unknown'
               and a.action in ('mute', 'ban', 'auto_mute')
               and (a.expires_at is null or a.expires_at > now())
           )"#,
    )
    .bind(chat_id)
    .bind(target_user_id)
    .fetch_one(&mut **tx)
    .await?;
    if unknown_restriction {
        anyhow::bail!(
            "предыдущее действие имеет неопределённый результат; сначала проверь статус Telegram вручную"
        );
    }

    let active_restriction: Option<(i64, String)> = sqlx::query_as(
        r#"select id, action from manual_moderation_actions
           where chat_id = $1 and target_user_id = $2 and status = 'applied'
             and action in ('mute', 'ban', 'auto_mute')
             and (expires_at is null or expires_at > now())
           order by created_at desc limit 1"#,
    )
    .bind(chat_id)
    .bind(target_user_id)
    .fetch_optional(&mut **tx)
    .await?;

    if action == "mute"
        && active_restriction
            .as_ref()
            .is_some_and(|(_, current)| current == "ban")
    {
        anyhow::bail!("пользователь забанен; сначала сними бан командой /unban");
    }
    if action == "auto_mute" && active_restriction.is_some() {
        return Ok(None);
    }

    let expires_at = expires_at.or_else(|| (action == "warn").then(|| Utc::now() + WARNING_TTL));
    let action_id: i64 = sqlx::query_scalar(
        r#"insert into manual_moderation_actions
               (batch_id, chat_id, target_user_id, actor_user_id, action, reason,
                status, expires_at, processing_lease_expires_at,
                supersedes_action_id, automatic)
           values ($1, $2, $3, $4, $5, $6, $11, $7,
                   case when $11 = 'processing' then now() + $8::interval else null end, $9, $10)
           returning id"#,
    )
    .bind(batch_id)
    .bind(chat_id)
    .bind(target_user_id)
    .bind(actor_user_id)
    .bind(action)
    .bind(reason)
    .bind(expires_at)
    .bind(format!("{} seconds", ACTION_LEASE.num_seconds()))
    .bind(active_restriction.map(|(id, _)| id))
    .bind(automatic)
    .bind(initial_status)
    .fetch_one(&mut **tx)
    .await?;

    let operation_status = if initial_status == "processing" {
        "processing"
    } else {
        "pending"
    };
    sqlx::query(
        r#"insert into manual_moderation_operations
               (batch_id, action_id, chat_id, target_user_id, actor_user_id,
                operation_kind, status, lease_expires_at)
           values ($1, $2, $3, $4, $5, 'apply', $6,
                   case when $6 = 'processing' then now() + $7::interval else null end)"#,
    )
    .bind(batch_id)
    .bind(action_id)
    .bind(chat_id)
    .bind(target_user_id)
    .bind(actor_user_id)
    .bind(operation_status)
    .bind(format!("{} seconds", ACTION_LEASE.num_seconds()))
    .execute(&mut **tx)
    .await?;

    insert_event(
        tx,
        batch_id,
        Some(action_id),
        chat_id,
        Some(target_user_id),
        actor_user_id,
        "requested",
        reason,
        None,
    )
    .await?;
    let prepared = sqlx::query_as::<_, PreparedAction>(
        r#"select id, status, expires_at
           from manual_moderation_actions where id = $1"#,
    )
    .bind(action_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(Some(prepared))
}

pub async fn mark_action_succeeded(pool: &PgPool, action_id: i64) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let action = load_action(&mut tx, action_id).await?;
    let operation_changed = sqlx::query(
        r#"update manual_moderation_operations
           set status = 'succeeded', lease_expires_at = null, updated_at = now()
           where action_id = $1 and operation_kind = 'apply' and status = 'processing'"#,
    )
    .bind(action_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if operation_changed != 1 {
        anyhow::bail!("apply operation left processing state before Telegram confirmation");
    }
    if let Some(previous_id) = action.supersedes_action_id {
        sqlx::query(
            "update manual_moderation_actions set status = 'superseded' where id = $1 and status = 'applied'",
        )
        .bind(previous_id)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            action.batch_id,
            Some(previous_id),
            action.chat_id,
            Some(action.target_user_id),
            action.actor_user_id,
            "superseded",
            action.reason.as_deref(),
            Some("replaced by a newer confirmed restriction"),
        )
        .await?;
    }
    let changed = sqlx::query(
        r#"update manual_moderation_actions
           set status = 'applied', processing_lease_expires_at = null
           where id = $1 and status in ('pending', 'processing')"#,
    )
    .bind(action_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed != 1 {
        anyhow::bail!("action left processing state before Telegram confirmation");
    }
    insert_event(
        &mut tx,
        action.batch_id,
        Some(action_id),
        action.chat_id,
        Some(action.target_user_id),
        action.actor_user_id,
        "applied",
        action.reason.as_deref(),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn mark_action_failed(
    pool: &PgPool,
    action_id: i64,
    unknown: bool,
    details: &str,
) -> anyhow::Result<()> {
    let status = if unknown { "unknown" } else { "failed" };
    let mut tx = pool.begin().await?;
    let action = load_action(&mut tx, action_id).await?;
    let operation_changed = sqlx::query(
        r#"update manual_moderation_operations
           set status = $2, lease_expires_at = null, details = $3, updated_at = now()
           where action_id = $1 and operation_kind = 'apply' and status = 'processing'"#,
    )
    .bind(action_id)
    .bind(status)
    .bind(details)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if operation_changed != 1 {
        anyhow::bail!("apply operation left processing state before Telegram outcome");
    }
    sqlx::query(
        "update manual_moderation_actions set status = $2, processing_lease_expires_at = null where id = $1 and status in ('pending', 'processing')",
    )
    .bind(action_id)
    .bind(status)
    .execute(&mut *tx)
    .await?;
    insert_event(
        &mut tx,
        action.batch_id,
        Some(action_id),
        action.chat_id,
        Some(action.target_user_id),
        action.actor_user_id,
        status,
        action.reason.as_deref(),
        Some(details),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

#[allow(dead_code)] // Kept for existing integrations and moderation tooling.
pub async fn add_warning(
    pool: &PgPool,
    batch_id: i64,
    chat_id: i64,
    target_user_id: i64,
    actor_user_id: i64,
    ttl: Duration,
    reason: Option<&str>,
) -> anyhow::Result<WarningResult> {
    add_warnings_batch(
        pool,
        batch_id,
        chat_id,
        &[target_user_id],
        actor_user_id,
        ttl,
        reason,
    )
    .await?
    .into_iter()
    .next()
    .ok_or_else(|| anyhow::anyhow!("warning batch returned no target outcome"))
}

pub async fn add_warnings_batch(
    pool: &PgPool,
    batch_id: i64,
    chat_id: i64,
    target_user_ids: &[i64],
    actor_user_id: i64,
    ttl: Duration,
    reason: Option<&str>,
) -> anyhow::Result<Vec<WarningResult>> {
    let mut sorted_ids = target_user_ids.to_vec();
    sorted_ids.sort_unstable();
    sorted_ids.dedup();
    if sorted_ids.len() != target_user_ids.len() {
        anyhow::bail!("warning batch contains duplicate targets");
    }
    let mut tx = pool.begin().await?;
    for target_user_id in &sorted_ids {
        lock_target(&mut tx, chat_id, *target_user_id).await?;
    }
    let mut outcomes = Vec::with_capacity(target_user_ids.len());
    for target_user_id in target_user_ids {
        expire_target_actions(&mut tx, chat_id, *target_user_id).await?;
        let existing_action_id: Option<i64> = sqlx::query_scalar(
            r#"select id from manual_moderation_actions
               where batch_id = $1 and target_user_id = $2 and action = 'warn'
               order by id desc limit 1"#,
        )
        .bind(batch_id)
        .bind(target_user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let action_id = if let Some(action_id) = existing_action_id {
            action_id
        } else {
            let conflicting_action: bool = sqlx::query_scalar(
                r#"select exists (
                     select 1 from manual_moderation_actions
                     where chat_id = $1 and target_user_id = $2
                       and status in ('pending', 'processing')
                     union all
                     select 1 from manual_moderation_operations
                     where chat_id = $1 and target_user_id = $2
                       and status in ('pending', 'processing')
                   )"#,
            )
            .bind(chat_id)
            .bind(target_user_id)
            .fetch_one(&mut *tx)
            .await?;
            if conflicting_action {
                anyhow::bail!(
                    "у пользователя {target_user_id} уже выполняется другая команда модерации"
                );
            }
            let unknown_restriction: bool = sqlx::query_scalar(
                r#"select exists (
                     select 1 from manual_moderation_actions
                     where chat_id = $1 and target_user_id = $2 and status = 'unknown'
                       and action in ('mute', 'ban', 'auto_mute')
                       and (expires_at is null or expires_at > now())
                     union all
                     select 1 from manual_moderation_operations o
                     join manual_moderation_actions a on a.id = o.action_id
                     where o.chat_id = $1 and o.target_user_id = $2 and o.status = 'unknown'
                       and a.action in ('mute', 'ban', 'auto_mute')
                       and (a.expires_at is null or a.expires_at > now())
                   )"#,
            )
            .bind(chat_id)
            .bind(target_user_id)
            .fetch_one(&mut *tx)
            .await?;
            if unknown_restriction {
                anyhow::bail!(
                    "у пользователя {target_user_id} есть действие с неопределённым исходом"
                );
            }
            let action_id: i64 = sqlx::query_scalar(
                r#"insert into manual_moderation_actions
                       (batch_id, chat_id, target_user_id, actor_user_id, action, reason,
                        status, expires_at)
                   values ($1, $2, $3, $4, 'warn', $5, 'applied', now() + $6::interval)
                   returning id"#,
            )
            .bind(batch_id)
            .bind(chat_id)
            .bind(target_user_id)
            .bind(actor_user_id)
            .bind(reason)
            .bind(format!("{} seconds", ttl.as_secs()))
            .fetch_one(&mut *tx)
            .await?;
            for event in ["requested", "applied"] {
                insert_event(
                    &mut tx,
                    batch_id,
                    Some(action_id),
                    chat_id,
                    Some(*target_user_id),
                    actor_user_id,
                    event,
                    reason,
                    None,
                )
                .await?;
            }
            action_id
        };
        let active_count: i64 = sqlx::query_scalar(
            r#"select count(*) from manual_moderation_actions
               where chat_id = $1 and target_user_id = $2 and action = 'warn'
                 and status = 'applied' and (expires_at is null or expires_at > now())"#,
        )
        .bind(chat_id)
        .bind(target_user_id)
        .fetch_one(&mut *tx)
        .await?;
        let has_restriction: bool = sqlx::query_scalar(
            r#"select exists (
                 select 1 from manual_moderation_actions
                 where chat_id = $1 and target_user_id = $2 and action in ('mute', 'ban', 'auto_mute')
                   and status = 'applied' and (expires_at is null or expires_at > now())
               )"#,
        )
        .bind(chat_id)
        .bind(target_user_id)
        .fetch_one(&mut *tx)
        .await?;
        outcomes.push(WarningResult {
            action_id,
            active_count,
            should_escalate: should_escalate_warning(active_count, has_restriction),
        });
    }
    tx.commit().await?;
    Ok(outcomes)
}

pub async fn active_warning_count(
    pool: &PgPool,
    chat_id: i64,
    target_user_id: i64,
) -> anyhow::Result<i64> {
    sqlx::query_scalar(
        r#"select count(*) from manual_moderation_actions
           where chat_id = $1 and target_user_id = $2 and action = 'warn'
             and status = 'applied' and (expires_at is null or expires_at > now())"#,
    )
    .bind(chat_id)
    .bind(target_user_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_warnings(
    pool: &PgPool,
    chat_id: i64,
    target_user_id: i64,
    limit: i64,
) -> anyhow::Result<Vec<ActionRecord>> {
    sqlx::query_as(
        r#"select id, batch_id, chat_id, target_user_id, actor_user_id, action,
                  reason, case when status = 'applied' and expires_at <= now() then 'expired' else status end as status,
                  created_at, expires_at, supersedes_action_id, automatic
           from manual_moderation_actions
           where chat_id = $1 and target_user_id = $2 and action = 'warn'
           order by created_at desc, id desc limit $3"#,
    )
    .bind(chat_id)
    .bind(target_user_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_actions(
    pool: &PgPool,
    chat_id: i64,
    target_user_id: Option<i64>,
    limit: i64,
) -> anyhow::Result<Vec<ActionRecord>> {
    sqlx::query_as(
        r#"select id, batch_id, chat_id, target_user_id, actor_user_id, action,
                  reason, case when status = 'applied' and expires_at <= now() then 'expired' else status end as status,
                  created_at, expires_at, supersedes_action_id, automatic
           from manual_moderation_actions
           where chat_id = $1 and ($2::bigint is null or target_user_id = $2)
           order by created_at desc, id desc limit $3"#,
    )
    .bind(chat_id)
    .bind(target_user_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn revoke_warnings(
    pool: &PgPool,
    batch_id: i64,
    chat_id: i64,
    target_user_id: i64,
    actor_user_id: i64,
    selection: WarningSelection,
    revoke_reason: Option<&str>,
) -> anyhow::Result<Vec<i64>> {
    let (warn_id, warn_all) = match selection {
        WarningSelection::Latest => (None, false),
        WarningSelection::All => (None, true),
        WarningSelection::Id(warn_id) => (Some(warn_id), false),
    };
    let mut tx = pool.begin().await?;
    lock_target(&mut tx, chat_id, target_user_id).await?;
    let revoked: Vec<(i64, Option<String>)> = sqlx::query_as(
        r#"update manual_moderation_actions
           set status = 'revoked'
           where chat_id = $1 and target_user_id = $2 and action = 'warn'
             and status = 'applied' and (expires_at is null or expires_at > now())
             and (
                 ($3::bigint is not null and id = $3)
                 or ($3::bigint is null and $4 and true)
                 or ($3::bigint is null and not $4 and id = (
                     select id from manual_moderation_actions
                     where chat_id = $1 and target_user_id = $2 and action = 'warn'
                       and status = 'applied' and (expires_at is null or expires_at > now())
                     order by created_at desc, id desc limit 1
                 ))
             )
           returning id, reason"#,
    )
    .bind(chat_id)
    .bind(target_user_id)
    .bind(warn_id)
    .bind(warn_all)
    .fetch_all(&mut *tx)
    .await?;
    for (action_id, reason) in &revoked {
        insert_event(
            &mut tx,
            batch_id,
            Some(*action_id),
            chat_id,
            Some(target_user_id),
            actor_user_id,
            "revoked",
            revoke_reason.or(reason.as_deref()),
            Some("revoked by /unwarn"),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(revoked.into_iter().map(|(id, _)| id).collect())
}

pub async fn active_restriction_action(
    pool: &PgPool,
    chat_id: i64,
    target_user_id: i64,
) -> anyhow::Result<Option<String>> {
    sqlx::query_scalar(
        r#"select action from manual_moderation_actions
           where chat_id = $1 and target_user_id = $2 and status = 'applied'
             and action in ('mute', 'ban', 'auto_mute')
             and (expires_at is null or expires_at > now())
           order by created_at desc, id desc limit 1"#,
    )
    .bind(chat_id)
    .bind(target_user_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn latest_batch(
    pool: &PgPool,
    chat_id: i64,
    actor_user_id: i64,
) -> anyhow::Result<Option<i64>> {
    sqlx::query_scalar(
        r#"select b.id
           from manual_moderation_batches b
           where b.chat_id = $1 and b.actor_user_id = $2
             and b.command in ('mute', 'ban', 'warn', 'unmute', 'unban', 'unwarn')
             and exists (
               select 1 from manual_moderation_events e
               where e.batch_id = b.id and e.action_id is not null
                 and e.event in ('applied', 'revoked')
             )
           order by b.created_at desc, b.id desc limit 1"#,
    )
    .bind(chat_id)
    .bind(actor_user_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn actions_in_batch(pool: &PgPool, batch_id: i64) -> anyhow::Result<Vec<ActionRecord>> {
    sqlx::query_as(
        r#"select id, batch_id, chat_id, target_user_id, actor_user_id, action,
                  reason, case when status = 'applied' and expires_at <= now() then 'expired' else status end as status,
                  created_at, expires_at, supersedes_action_id, automatic
           from manual_moderation_actions a
           where a.batch_id = $1 or exists (
             select 1 from manual_moderation_events e
             where e.batch_id = $1 and e.action_id = a.id and e.event = 'revoked'
           )
           order by case when action = 'warn' then 1 else 0 end, id desc"#,
    )
    .bind(batch_id)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn claim_undo_action(
    pool: &PgPool,
    undo_batch_id: i64,
    target_batch_id: i64,
    action_id: i64,
    actor_user_id: i64,
) -> anyhow::Result<UndoClaim> {
    let mut tx = pool.begin().await?;
    let target: (i64, i64) = sqlx::query_as(
        "select chat_id, target_user_id from manual_moderation_actions where id = $1",
    )
    .bind(action_id)
    .fetch_one(&mut *tx)
    .await?;
    lock_target(&mut tx, target.0, target.1).await?;
    let current = load_action(&mut tx, action_id).await?;
    if current
        .expires_at
        .is_some_and(|expires| expires <= Utc::now())
    {
        sqlx::query("update manual_moderation_actions set status = 'expired' where id = $1")
            .bind(action_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(UndoClaim::Expired);
    }

    let was_revoked_by_target_batch: bool = sqlx::query_scalar(
        "select exists (select 1 from manual_moderation_events where batch_id = $1 and action_id = $2 and event = 'revoked')",
    )
    .bind(target_batch_id)
    .bind(action_id)
    .fetch_one(&mut *tx)
    .await?;
    let already_undone: bool = sqlx::query_scalar(
        r#"select exists (
             select 1
             from manual_moderation_events undo_event
             join manual_moderation_batches undo_batch on undo_batch.id = undo_event.batch_id
             join manual_moderation_batches target_batch on target_batch.id = $2
             where undo_event.action_id = $1
               and undo_event.actor_user_id = $3
               and undo_batch.command = 'undo'
               and undo_batch.created_at > target_batch.created_at
               and undo_event.event in ('applied', 'revoked')
           )"#,
    )
    .bind(action_id)
    .bind(target_batch_id)
    .bind(actor_user_id)
    .fetch_one(&mut *tx)
    .await?;
    if already_undone {
        tx.commit().await?;
        return Ok(UndoClaim::Conflict);
    }
    if current.status != "applied" && !(current.status == "revoked" && was_revoked_by_target_batch)
    {
        tx.commit().await?;
        return Ok(UndoClaim::Conflict);
    }

    if current.status == "revoked" && current.action == "warn" {
        sqlx::query("update manual_moderation_actions set status = 'applied' where id = $1 and status = 'revoked'")
            .bind(action_id)
            .execute(&mut *tx)
            .await?;
        insert_event(
            &mut tx,
            undo_batch_id,
            Some(action_id),
            current.chat_id,
            Some(current.target_user_id),
            actor_user_id,
            "applied",
            current.reason.as_deref(),
            Some("restored by /undo"),
        )
        .await?;
        tx.commit().await?;
        return Ok(UndoClaim::WarningRestored);
    }

    if current.action == "warn" {
        sqlx::query("update manual_moderation_actions set status = 'revoked' where id = $1")
            .bind(action_id)
            .execute(&mut *tx)
            .await?;
        insert_event(
            &mut tx,
            undo_batch_id,
            Some(action_id),
            current.chat_id,
            Some(current.target_user_id),
            actor_user_id,
            "revoked",
            current.reason.as_deref(),
            Some("revoked by /undo"),
        )
        .await?;
        tx.commit().await?;
        return Ok(UndoClaim::WarningRevoked);
    }

    let existing_operation: Option<(String, String)> = sqlx::query_as(
        r#"select operation_kind, status from manual_moderation_operations
           where batch_id = $1 and action_id = $2
             and operation_kind in ('undo', 'restore')
           order by id desc limit 1"#,
    )
    .bind(undo_batch_id)
    .bind(action_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((operation_kind, status)) = existing_operation {
        tx.commit().await?;
        return Ok(if status == "pending" {
            if operation_kind == "restore" {
                UndoClaim::RestrictionReapplyClaimed
            } else {
                UndoClaim::RestrictionClaimed
            }
        } else {
            UndoClaim::Conflict
        });
    }

    let inflight_operation: bool = sqlx::query_scalar(
        r#"select exists (
             select 1 from manual_moderation_operations
             where chat_id = $1 and target_user_id = $2
               and status in ('pending', 'processing')
             union all
             select 1 from manual_moderation_actions
             where chat_id = $1 and target_user_id = $2
               and status in ('pending', 'processing')
           )"#,
    )
    .bind(current.chat_id)
    .bind(current.target_user_id)
    .fetch_one(&mut *tx)
    .await?;
    if inflight_operation {
        tx.commit().await?;
        return Ok(UndoClaim::Conflict);
    }

    if current.status == "revoked" {
        let active_restriction: bool = sqlx::query_scalar(
            r#"select exists (
                 select 1 from manual_moderation_actions
                 where chat_id = $1 and target_user_id = $2 and status = 'applied'
                   and action in ('mute', 'ban', 'auto_mute')
                   and (expires_at is null or expires_at > now())
               )"#,
        )
        .bind(current.chat_id)
        .bind(current.target_user_id)
        .fetch_one(&mut *tx)
        .await?;
        if active_restriction {
            tx.commit().await?;
            return Ok(UndoClaim::Conflict);
        }
        sqlx::query(
            r#"insert into manual_moderation_operations
                   (batch_id, action_id, chat_id, target_user_id, actor_user_id,
                    operation_kind, status)
               values ($1, $2, $3, $4, $5, 'restore', 'pending')"#,
        )
        .bind(undo_batch_id)
        .bind(action_id)
        .bind(current.chat_id)
        .bind(current.target_user_id)
        .bind(actor_user_id)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            undo_batch_id,
            Some(action_id),
            current.chat_id,
            Some(current.target_user_id),
            actor_user_id,
            "requested",
            current.reason.as_deref(),
            Some("restore revoked restriction requested"),
        )
        .await?;
        tx.commit().await?;
        return Ok(UndoClaim::RestrictionReapplyClaimed);
    }

    let latest_active_id: Option<i64> = sqlx::query_scalar(
        r#"select id from manual_moderation_actions
           where chat_id = $1 and target_user_id = $2 and status = 'applied'
             and action in ('mute', 'ban', 'auto_mute')
             and (expires_at is null or expires_at > now())
           order by created_at desc, id desc limit 1"#,
    )
    .bind(current.chat_id)
    .bind(current.target_user_id)
    .fetch_optional(&mut *tx)
    .await?;
    if latest_active_id != Some(action_id) {
        tx.commit().await?;
        return Ok(UndoClaim::Conflict);
    }

    sqlx::query(
        r#"insert into manual_moderation_operations
               (batch_id, action_id, chat_id, target_user_id, actor_user_id,
                operation_kind, status)
           values ($1, $2, $3, $4, $5, 'undo', 'pending')"#,
    )
    .bind(undo_batch_id)
    .bind(action_id)
    .bind(current.chat_id)
    .bind(current.target_user_id)
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await?;
    insert_event(
        &mut tx,
        undo_batch_id,
        Some(action_id),
        current.chat_id,
        Some(current.target_user_id),
        actor_user_id,
        "requested",
        current.reason.as_deref(),
        Some("undo requested"),
    )
    .await?;
    tx.commit().await?;
    Ok(UndoClaim::RestrictionClaimed)
}

pub async fn start_undo_operation(
    pool: &PgPool,
    undo_batch_id: i64,
    action_id: i64,
) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let (chat_id, target_user_id): (i64, i64) = sqlx::query_as(
        "select chat_id, target_user_id from manual_moderation_actions where id = $1",
    )
    .bind(action_id)
    .fetch_one(&mut *tx)
    .await?;
    lock_target(&mut tx, chat_id, target_user_id).await?;
    let changed = sqlx::query(
        r#"update manual_moderation_operations
           set status = 'processing', lease_expires_at = now() + $3::interval,
               updated_at = now()
           where batch_id = $1 and action_id = $2
             and operation_kind in ('undo', 'restore') and status = 'pending'"#,
    )
    .bind(undo_batch_id)
    .bind(action_id)
    .bind(format!("{} seconds", ACTION_LEASE.num_seconds()))
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 1 {
        sqlx::query(
            r#"update manual_moderation_batches
               set processing_lease_expires_at = now() + interval '5 minutes', updated_at = now()
               where id = $1"#,
        )
        .bind(undo_batch_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(changed == 1)
}

pub async fn undo_previous_action(
    pool: &PgPool,
    current_action_id: i64,
) -> anyhow::Result<Option<ActionRecord>> {
    let action = sqlx::query_as::<_, ActionRecord>(
        r#"select old.id, old.batch_id, old.chat_id, old.target_user_id, old.actor_user_id,
                  old.action, old.reason, old.status, old.created_at, old.expires_at,
                  old.supersedes_action_id, old.automatic
           from manual_moderation_actions current
           join manual_moderation_actions old on old.id = current.supersedes_action_id
           where current.id = $1 and old.status = 'superseded'
             and (old.expires_at is null or old.expires_at > now())"#,
    )
    .bind(current_action_id)
    .fetch_optional(pool)
    .await?;
    Ok(action)
}

pub async fn finish_undo(
    pool: &PgPool,
    undo_batch_id: i64,
    action_id: i64,
    actor_user_id: i64,
    succeeded: bool,
    unknown: bool,
    reapply: bool,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let action = load_action(&mut tx, action_id).await?;
    let operation_status = if succeeded {
        "succeeded"
    } else if unknown {
        "unknown"
    } else {
        "failed"
    };
    let operation_kind = if reapply { "restore" } else { "undo" };
    let operation_changed = sqlx::query(
        r#"update manual_moderation_operations
           set status = $3, lease_expires_at = null, updated_at = now()
           where batch_id = $1 and action_id = $2
             and operation_kind = $4 and status = 'processing'"#,
    )
    .bind(undo_batch_id)
    .bind(action_id)
    .bind(operation_status)
    .bind(operation_kind)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if operation_changed != 1 {
        anyhow::bail!("undo operation left processing state before Telegram outcome");
    }
    if succeeded || unknown {
        let (expected_action_status, action_status) = if unknown {
            (if reapply { "revoked" } else { "applied" }, "unknown")
        } else if reapply {
            ("revoked", "applied")
        } else {
            ("applied", "revoked")
        };
        let action_changed = sqlx::query(
            "update manual_moderation_actions set status = $3, processing_lease_expires_at = null where id = $1 and status = $2",
        )
        .bind(action_id)
        .bind(expected_action_status)
        .bind(action_status)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if action_changed != 1 {
            anyhow::bail!("moderation action changed before undo confirmation");
        }
    }
    if succeeded && !reapply {
        let previous_id: Option<i64> = sqlx::query_scalar(
            "select supersedes_action_id from manual_moderation_actions where id = $1",
        )
        .bind(action_id)
        .fetch_one(&mut *tx)
        .await?;
        if let Some(previous_id) = previous_id {
            sqlx::query(
                r#"update manual_moderation_actions
                   set status = case when expires_at is not null and expires_at <= now() then 'expired' else 'applied' end
                   where id = $1 and status = 'superseded'"#,
            )
            .bind(previous_id)
            .execute(&mut *tx)
            .await?;
        }
    }
    insert_event(
        &mut tx,
        undo_batch_id,
        Some(action_id),
        action.chat_id,
        Some(action.target_user_id),
        actor_user_id,
        if unknown {
            "unknown"
        } else if succeeded && reapply {
            "applied"
        } else if succeeded {
            "revoked"
        } else {
            "undo_skipped"
        },
        action.reason.as_deref(),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

#[allow(dead_code)] // Single-target API remains for external feature consumers.
pub async fn claim_restriction_revoke(
    pool: &PgPool,
    batch_id: i64,
    chat_id: i64,
    target_user_id: i64,
    actor_user_id: i64,
    expected_action: &str,
) -> anyhow::Result<Option<ActionRecord>> {
    let prepared = claim_restriction_revokes_batch(
        pool,
        batch_id,
        chat_id,
        &[target_user_id],
        actor_user_id,
        expected_action,
    )
    .await?
    .into_iter()
    .next()
    .flatten();
    let Some(prepared) = prepared.filter(|prepared| prepared.operation_status == "pending") else {
        return Ok(None);
    };
    if !start_restriction_revoke(pool, prepared.operation_id).await? {
        return Ok(None);
    }
    Ok(Some(prepared.action))
}

pub async fn claim_restriction_revokes_batch(
    pool: &PgPool,
    batch_id: i64,
    chat_id: i64,
    target_user_ids: &[i64],
    actor_user_id: i64,
    expected_action: &str,
) -> anyhow::Result<Vec<Option<PreparedRestrictionRevoke>>> {
    let mut sorted_ids = target_user_ids.to_vec();
    sorted_ids.sort_unstable();
    sorted_ids.dedup();
    if sorted_ids.len() != target_user_ids.len() {
        anyhow::bail!("restriction revoke batch contains duplicate targets");
    }
    let mut tx = pool.begin().await?;
    for target_user_id in &sorted_ids {
        lock_target(&mut tx, chat_id, *target_user_id).await?;
    }
    let mut prepared = Vec::with_capacity(target_user_ids.len());
    for target_user_id in target_user_ids {
        expire_target_actions(&mut tx, chat_id, *target_user_id).await?;

        let existing_operation: Option<(i64, String, i64)> = sqlx::query_as(
            r#"select id, status, action_id from manual_moderation_operations
               where batch_id = $1 and target_user_id = $2 and operation_kind = 'revoke'
               order by id desc limit 1"#,
        )
        .bind(batch_id)
        .bind(target_user_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((operation_id, operation_status, action_id)) = existing_operation {
            prepared.push(Some(PreparedRestrictionRevoke {
                operation_id,
                operation_status,
                action: load_action(&mut tx, action_id).await?,
            }));
            continue;
        }

        let inflight: bool = sqlx::query_scalar(
            r#"select exists (
                 select 1 from manual_moderation_actions
                 where chat_id = $1 and target_user_id = $2
                   and status in ('pending', 'processing')
                 union all
                 select 1 from manual_moderation_operations
                 where chat_id = $1 and target_user_id = $2
                   and status in ('pending', 'processing')
               )"#,
        )
        .bind(chat_id)
        .bind(target_user_id)
        .fetch_one(&mut *tx)
        .await?;
        if inflight {
            anyhow::bail!(
                "у пользователя {target_user_id} уже выполняется другая команда модерации"
            );
        }
        let unknown: bool = sqlx::query_scalar(
            r#"select exists (
                 select 1 from manual_moderation_actions
                 where chat_id = $1 and target_user_id = $2 and status = 'unknown'
                   and action in ('mute', 'ban', 'auto_mute')
                   and (expires_at is null or expires_at > now())
                 union all
                 select 1 from manual_moderation_operations o
                 join manual_moderation_actions a on a.id = o.action_id
                 where o.chat_id = $1 and o.target_user_id = $2 and o.status = 'unknown'
                   and a.action in ('mute', 'ban', 'auto_mute')
                   and (a.expires_at is null or a.expires_at > now())
               )"#,
        )
        .bind(chat_id)
        .bind(target_user_id)
        .fetch_one(&mut *tx)
        .await?;
        if unknown {
            anyhow::bail!(
                "у пользователя {target_user_id} есть ограничение с неопределённым исходом"
            );
        }
        let action = sqlx::query_as::<_, ActionRecord>(
            r#"select id, batch_id, chat_id, target_user_id, actor_user_id, action, reason,
                      status, created_at, expires_at, supersedes_action_id, automatic
               from manual_moderation_actions
               where chat_id = $1 and target_user_id = $2 and status = 'applied'
                 and action in ('mute', 'ban', 'auto_mute')
                 and (expires_at is null or expires_at > now())
               order by created_at desc, id desc limit 1 for update"#,
        )
        .bind(chat_id)
        .bind(target_user_id)
        .fetch_optional(&mut *tx)
        .await?
        .filter(|action| {
            expected_action == "any"
                || (expected_action == "mute" && action.action != "ban")
                || action.action == expected_action
        });
        if let Some(action) = action {
            let operation_id: i64 = sqlx::query_scalar(
                r#"insert into manual_moderation_operations
                       (batch_id, action_id, chat_id, target_user_id, actor_user_id,
                        operation_kind, status)
                   values ($1, $2, $3, $4, $5, 'revoke', 'pending')
                   returning id"#,
            )
            .bind(batch_id)
            .bind(action.id)
            .bind(chat_id)
            .bind(target_user_id)
            .bind(actor_user_id)
            .fetch_one(&mut *tx)
            .await?;
            insert_event(
                &mut tx,
                batch_id,
                Some(action.id),
                chat_id,
                Some(*target_user_id),
                actor_user_id,
                "requested",
                action.reason.as_deref(),
                Some("restriction revoke requested"),
            )
            .await?;
            prepared.push(Some(PreparedRestrictionRevoke {
                operation_id,
                operation_status: "pending".to_string(),
                action,
            }));
        } else {
            prepared.push(None);
        }
    }
    tx.commit().await?;
    Ok(prepared)
}

pub async fn start_restriction_revoke(pool: &PgPool, operation_id: i64) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let (batch_id, chat_id, target_user_id): (i64, i64, i64) = sqlx::query_as(
        "select batch_id, chat_id, target_user_id from manual_moderation_operations where id = $1",
    )
    .bind(operation_id)
    .fetch_one(&mut *tx)
    .await?;
    lock_target(&mut tx, chat_id, target_user_id).await?;
    let changed = sqlx::query(
        r#"update manual_moderation_operations
           set status = 'processing', lease_expires_at = now() + $2::interval,
               updated_at = now()
           where id = $1 and operation_kind = 'revoke' and status = 'pending'"#,
    )
    .bind(operation_id)
    .bind(format!("{} seconds", ACTION_LEASE.num_seconds()))
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 1 {
        sqlx::query(
            r#"update manual_moderation_batches
               set processing_lease_expires_at = now() + interval '5 minutes', updated_at = now()
               where id = $1"#,
        )
        .bind(batch_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(changed == 1)
}

pub async fn finish_restriction_revoke(
    pool: &PgPool,
    batch_id: i64,
    action_id: i64,
    actor_user_id: i64,
    succeeded: bool,
    unknown: bool,
    revoke_reason: Option<&str>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let action = load_action(&mut tx, action_id).await?;
    let status = if succeeded {
        "succeeded"
    } else if unknown {
        "unknown"
    } else {
        "failed"
    };
    let operation_changed = sqlx::query(
        r#"update manual_moderation_operations
           set status = $2, lease_expires_at = null, details = $3, updated_at = now()
           where batch_id = $1 and action_id = $4
             and operation_kind = 'revoke' and status = 'processing'"#,
    )
    .bind(batch_id)
    .bind(status)
    .bind(revoke_reason)
    .bind(action_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if operation_changed != 1 {
        anyhow::bail!("revoke operation left processing state before Telegram outcome");
    }
    if succeeded || unknown {
        let action_status = if succeeded { "revoked" } else { "unknown" };
        let action_changed = sqlx::query(
            r#"update manual_moderation_actions
               set status = $2, processing_lease_expires_at = null
               where id = $1 and status = 'applied'"#,
        )
        .bind(action_id)
        .bind(action_status)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if action_changed != 1 {
            anyhow::bail!("restriction action changed before revoke confirmation");
        }
    }
    insert_event(
        &mut tx,
        batch_id,
        Some(action_id),
        action.chat_id,
        Some(action.target_user_id),
        actor_user_id,
        if succeeded {
            "revoked"
        } else if unknown {
            "unknown"
        } else {
            "failed"
        },
        revoke_reason.or(action.reason.as_deref()),
        Some("restriction revoke"),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn find_username(
    pool: &PgPool,
    chat_id: i64,
    username: &str,
) -> anyhow::Result<Option<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        r#"select distinct p.telegram_user_id
           from telegram_user_profiles p
           join telegram_chat_users u on u.telegram_user_id = p.telegram_user_id
           where u.chat_id = $1 and lower(p.username) = $2
           order by p.telegram_user_id limit 2"#,
    )
    .bind(chat_id)
    .bind(username.to_ascii_lowercase())
    .fetch_all(pool)
    .await?;
    match rows.as_slice() {
        [(user_id,)] => Ok(Some(*user_id)),
        [] => Ok(None),
        _ => anyhow::bail!("username неоднозначен; используй reply или Telegram ID"),
    }
}

pub async fn known_chat_user(pool: &PgPool, chat_id: i64, user_id: i64) -> anyhow::Result<bool> {
    sqlx::query_scalar(
        "select exists (select 1 from telegram_chat_users where chat_id = $1 and telegram_user_id = $2)",
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

async fn expire_target_actions(
    tx: &mut Transaction<'_, Postgres>,
    chat_id: i64,
    target_user_id: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"update manual_moderation_actions
           set status = 'expired'
           where chat_id = $1 and target_user_id = $2 and status = 'applied'
             and expires_at is not null and expires_at <= now()"#,
    )
    .bind(chat_id)
    .bind(target_user_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"update manual_moderation_actions
           set status = 'unknown', processing_lease_expires_at = null
           where chat_id = $1 and target_user_id = $2 and status = 'processing'
             and processing_lease_expires_at <= now()"#,
    )
    .bind(chat_id)
    .bind(target_user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn lock_target(
    tx: &mut Transaction<'_, Postgres>,
    chat_id: i64,
    target_user_id: i64,
) -> anyhow::Result<()> {
    sqlx::query("select pg_advisory_xact_lock(hashtextextended($1::text || ':' || $2::text, 0))")
        .bind(chat_id)
        .bind(target_user_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn load_action(
    tx: &mut Transaction<'_, Postgres>,
    action_id: i64,
) -> anyhow::Result<ActionRecord> {
    sqlx::query_as(
        r#"select id, batch_id, chat_id, target_user_id, actor_user_id, action,
                  reason, status, created_at, expires_at, supersedes_action_id, automatic
           from manual_moderation_actions where id = $1 for update"#,
    )
    .bind(action_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(Into::into)
}

#[allow(clippy::too_many_arguments)]
async fn insert_event(
    tx: &mut Transaction<'_, Postgres>,
    batch_id: i64,
    action_id: Option<i64>,
    chat_id: i64,
    target_user_id: Option<i64>,
    actor_user_id: i64,
    event: &str,
    reason: Option<&str>,
    details: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"insert into manual_moderation_events
               (batch_id, action_id, chat_id, target_user_id, actor_user_id, event, reason, details)
           values ($1, $2, $3, $4, $5, $6, $7, $8)"#,
    )
    .bind(batch_id)
    .bind(action_id)
    .bind(chat_id)
    .bind(target_user_id)
    .bind(actor_user_id)
    .bind(event)
    .bind(reason)
    .bind(details)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
