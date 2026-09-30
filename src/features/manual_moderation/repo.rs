use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use std::time::Duration;

use super::types::{WARNING_TTL, should_escalate_warning};

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
    pub is_new: bool,
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

pub async fn create_batch(
    pool: &PgPool,
    chat_id: i64,
    actor_user_id: i64,
    source_message_id: i32,
    command: &str,
) -> anyhow::Result<BatchCreation> {
    if let Some((id,)) = sqlx::query_as::<_, (i64,)>(
        r#"insert into manual_moderation_batches
               (chat_id, actor_user_id, source_message_id, command)
           values ($1, $2, $3, $4)
           on conflict (chat_id, source_message_id) do nothing
           returning id"#,
    )
    .bind(chat_id)
    .bind(actor_user_id)
    .bind(source_message_id)
    .bind(command)
    .fetch_optional(pool)
    .await?
    {
        return Ok(BatchCreation { id, is_new: true });
    }

    let (id,) = sqlx::query_as(
        "select id from manual_moderation_batches where chat_id = $1 and source_message_id = $2",
    )
    .bind(chat_id)
    .bind(source_message_id)
    .fetch_one(pool)
    .await?;
    Ok(BatchCreation { id, is_new: false })
}

pub async fn prepare_action(
    pool: &PgPool,
    request: ActionPreparation<'_>,
) -> anyhow::Result<Option<i64>> {
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
    let mut tx = pool.begin().await?;
    lock_target(&mut tx, chat_id, target_user_id).await?;
    expire_target_actions(&mut tx, chat_id, target_user_id).await?;

    let processing: Option<(i64,)> = sqlx::query_as(
        "select id from manual_moderation_actions where chat_id = $1 and target_user_id = $2 and status = 'processing'",
    )
    .bind(chat_id)
    .bind(target_user_id)
    .fetch_optional(&mut *tx)
    .await?;
    if processing.is_some() {
        anyhow::bail!("у пользователя уже выполняется другая команда модерации");
    }

    let unknown_restriction: bool = sqlx::query_scalar(
        r#"select exists (
             select 1 from manual_moderation_actions
             where chat_id = $1 and target_user_id = $2 and status = 'unknown'
               and action in ('mute', 'ban', 'auto_mute')
               and (expires_at is null or expires_at > now())
           )"#,
    )
    .bind(chat_id)
    .bind(target_user_id)
    .fetch_one(&mut *tx)
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
    .fetch_optional(&mut *tx)
    .await?;

    if action == "mute"
        && active_restriction
            .as_ref()
            .is_some_and(|(_, current)| current == "ban")
    {
        anyhow::bail!("пользователь забанен; сначала сними бан командой /unban");
    }
    if action == "auto_mute" && active_restriction.is_some() {
        tx.commit().await?;
        return Ok(None);
    }

    let expires_at = expires_at.or_else(|| (action == "warn").then(|| Utc::now() + WARNING_TTL));
    let action_id: i64 = sqlx::query_scalar(
        r#"insert into manual_moderation_actions
               (batch_id, chat_id, target_user_id, actor_user_id, action, reason,
                status, expires_at, processing_lease_expires_at,
                supersedes_action_id, automatic)
           values ($1, $2, $3, $4, $5, $6, 'processing', $7, now() + $8::interval, $9, $10)
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
    .fetch_one(&mut *tx)
    .await?;

    insert_event(
        &mut tx,
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
    tx.commit().await?;
    Ok(Some(action_id))
}

pub async fn mark_action_succeeded(pool: &PgPool, action_id: i64) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let action = load_action(&mut tx, action_id).await?;
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
           where id = $1 and status = 'processing'"#,
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
    sqlx::query(
        "update manual_moderation_actions set status = $2, processing_lease_expires_at = null where id = $1 and status = 'processing'",
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

pub async fn add_warning(
    pool: &PgPool,
    batch_id: i64,
    chat_id: i64,
    target_user_id: i64,
    actor_user_id: i64,
    ttl: Duration,
    reason: Option<&str>,
) -> anyhow::Result<WarningResult> {
    let mut tx = pool.begin().await?;
    lock_target(&mut tx, chat_id, target_user_id).await?;
    expire_target_actions(&mut tx, chat_id, target_user_id).await?;
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
    insert_event(
        &mut tx,
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
    insert_event(
        &mut tx,
        batch_id,
        Some(action_id),
        chat_id,
        Some(target_user_id),
        actor_user_id,
        "applied",
        reason,
        None,
    )
    .await?;
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
    tx.commit().await?;
    Ok(WarningResult {
        action_id,
        active_count,
        should_escalate: should_escalate_warning(active_count, has_restriction),
    })
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
    warn_id: Option<i64>,
    revoke_reason: Option<&str>,
) -> anyhow::Result<Vec<i64>> {
    let mut tx = pool.begin().await?;
    lock_target(&mut tx, chat_id, target_user_id).await?;
    let revoked: Vec<(i64, Option<String>)> = sqlx::query_as(
        r#"update manual_moderation_actions
           set status = 'revoked'
           where chat_id = $1 and target_user_id = $2 and action = 'warn'
             and status = 'applied' and (expires_at is null or expires_at > now())
             and ($3::bigint is null or id = $3)
           returning id, reason"#,
    )
    .bind(chat_id)
    .bind(target_user_id)
    .bind(warn_id)
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
            "update manual_moderation_actions set status = 'processing', processing_lease_expires_at = now() + $2::interval where id = $1 and status = 'revoked'",
        )
        .bind(action_id)
        .bind(format!("{} seconds", ACTION_LEASE.num_seconds()))
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

    let previous = if let Some(previous_id) = current.supersedes_action_id {
        let previous = load_action(&mut tx, previous_id).await?;
        (previous.status == "superseded"
            && previous
                .expires_at
                .is_none_or(|expires| expires > Utc::now()))
        .then_some(previous)
    } else {
        None
    };
    sqlx::query(
        "update manual_moderation_actions set status = 'processing', processing_lease_expires_at = now() + $2::interval where id = $1 and status = 'applied'",
    )
    .bind(action_id)
    .bind(format!("{} seconds", ACTION_LEASE.num_seconds()))
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
    if let Some(previous) = previous {
        sqlx::query(
            "update manual_moderation_actions set status = 'pending' where id = $1 and status = 'superseded'",
        )
        .bind(previous.id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(UndoClaim::RestrictionClaimed)
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
           where current.id = $1 and old.status = 'pending'"#,
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
    let status = match (succeeded, unknown, reapply) {
        (true, _, true) => "applied",
        (true, _, false) => "revoked",
        (false, true, _) => "unknown",
        (false, false, true) => "revoked",
        (false, false, false) => "applied",
    };
    sqlx::query(
        "update manual_moderation_actions set status = $2, processing_lease_expires_at = null where id = $1 and status = 'processing'",
    )
    .bind(action_id)
    .bind(status)
    .execute(&mut *tx)
    .await?;
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
                   where id = $1 and status = 'pending'"#,
            )
            .bind(previous_id)
            .execute(&mut *tx)
            .await?;
        }
    } else if !succeeded
        && !reapply
        && let Some(previous_id) = action.supersedes_action_id
    {
        sqlx::query(
            "update manual_moderation_actions set status = $2 where id = $1 and status = 'pending'",
        )
        .bind(previous_id)
        .bind(if unknown { "unknown" } else { "superseded" })
        .execute(&mut *tx)
        .await?;
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

pub async fn claim_restriction_revoke(
    pool: &PgPool,
    batch_id: i64,
    chat_id: i64,
    target_user_id: i64,
    actor_user_id: i64,
    expected_action: &str,
) -> anyhow::Result<Option<ActionRecord>> {
    let mut tx = pool.begin().await?;
    lock_target(&mut tx, chat_id, target_user_id).await?;
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
    .await?;
    let Some(action) = action.filter(|action| {
        expected_action == "any"
            || (expected_action == "mute" && action.action != "ban")
            || action.action == expected_action
    }) else {
        tx.commit().await?;
        return Ok(None);
    };
    sqlx::query(
        "update manual_moderation_actions set status = 'processing', processing_lease_expires_at = now() + $2::interval where id = $1",
    )
    .bind(action.id)
    .bind(format!("{} seconds", ACTION_LEASE.num_seconds()))
    .execute(&mut *tx)
    .await?;
    insert_event(
        &mut tx,
        batch_id,
        Some(action.id),
        chat_id,
        Some(target_user_id),
        actor_user_id,
        "requested",
        action.reason.as_deref(),
        Some("restriction revoke requested"),
    )
    .await?;
    tx.commit().await?;
    Ok(Some(action))
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
        "revoked"
    } else if unknown {
        "unknown"
    } else {
        "applied"
    };
    sqlx::query(
        "update manual_moderation_actions set status = $2, processing_lease_expires_at = null where id = $1 and status = 'processing'",
    )
    .bind(action_id)
    .bind(status)
    .execute(&mut *tx)
    .await?;
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
