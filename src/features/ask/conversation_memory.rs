use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, types::Json};
use std::collections::HashSet;

use crate::features::ask::profile::AgentProfile;

const MAX_STORED_TURN_CHARS: usize = 16_000;
const MAX_STORED_SOURCE_MESSAGE_IDS: usize = 256;
const MAX_STORED_SOURCE_URLS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationScope {
    pub instance_id: String,
    pub profile_key: String,
    pub chat_id: i64,
    /// Verified reply-chain root. Zero denotes a no-reply conversation, which
    /// is additionally partitioned by participant_user_id.
    pub thread_root_message_id: i32,
    pub participant_user_id: i64,
    /// Captured before generation; stale generations cannot write after /forget.
    pub memory_epoch: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationTurn {
    pub request_message_id: i32,
    pub reply_to_message_id: Option<i32>,
    pub user_text: String,
    pub assistant_text: String,
    pub source_message_ids: Vec<i32>,
    pub source_urls: Vec<String>,
}

#[derive(Debug, FromRow)]
struct ConversationTurnRow {
    command_message_id: i32,
    reply_to_message_id: Option<i32>,
    user_text: String,
    assistant_text: String,
    source_message_ids: Json<Vec<i32>>,
    source_urls: Json<Vec<String>>,
}

pub async fn load_recent(
    pool: &PgPool,
    scope: &ConversationScope,
    exclude_command_message_id: i32,
    profile: &AgentProfile,
    allowed_reply_to_ids: Option<&HashSet<i32>>,
) -> anyhow::Result<Vec<ConversationTurn>> {
    if !profile.memory.enabled {
        return Ok(Vec::new());
    }
    let _ = purge_expired(pool).await?;
    let rows = sqlx::query_as::<_, ConversationTurnRow>(
        r#"
        select command_message_id, reply_to_message_id, user_text, assistant_text,
               source_message_ids, source_urls
        from (
            select command_message_id, reply_to_message_id, user_text, assistant_text,
                   source_message_ids, source_urls, created_at
            from ask_conversation_memory
            where instance_id = $1 and profile_key = $2 and chat_id = $3
              and thread_root_message_id = $4 and participant_user_id = $5
              and command_message_id <> $6
              and ($7::integer[] is null or reply_to_message_id = any($7))
              and expires_at > now()
            order by created_at desc, command_message_id desc
            limit $8
        ) recent
        order by created_at asc, command_message_id asc
        "#,
    )
    .bind(&scope.instance_id)
    .bind(&scope.profile_key)
    .bind(scope.chat_id)
    .bind(scope.thread_root_message_id)
    .bind(scope.participant_user_id)
    .bind(exclude_command_message_id)
    .bind(allowed_reply_to_ids.map(|ids| ids.iter().copied().collect::<Vec<_>>()))
    .bind(i64::try_from(profile.memory.max_turns).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await?;
    let turns = rows
        .into_iter()
        .map(|row| ConversationTurn {
            request_message_id: row.command_message_id,
            reply_to_message_id: row.reply_to_message_id,
            user_text: row.user_text,
            assistant_text: row.assistant_text,
            source_message_ids: row.source_message_ids.0,
            source_urls: row.source_urls.0,
        })
        .collect();
    Ok(bound_turns(turns, profile.memory.max_context_chars))
}

pub async fn append_turn(
    pool: &PgPool,
    scope: &ConversationScope,
    turn: &ConversationTurn,
    retention_days: u32,
    max_turns: usize,
) -> anyhow::Result<bool> {
    let max_question_chars = MAX_STORED_TURN_CHARS / 3;
    let user_text = truncate_text(&turn.user_text, max_question_chars);
    let assistant_text = truncate_text(
        &turn.assistant_text,
        MAX_STORED_TURN_CHARS - user_text.chars().count(),
    );
    let source_message_ids = turn
        .source_message_ids
        .iter()
        .copied()
        .take(MAX_STORED_SOURCE_MESSAGE_IDS)
        .collect::<Vec<_>>();
    let source_urls = turn
        .source_urls
        .iter()
        .take(MAX_STORED_SOURCE_URLS)
        .map(|url| truncate_text(url, 2048))
        .collect::<Vec<_>>();
    let mut transaction = pool.begin().await?;
    lock_scope(&mut transaction, scope).await?;
    let current_epoch = read_epoch_in_transaction(&mut transaction, scope).await?;
    if current_epoch != scope.memory_epoch {
        transaction.rollback().await?;
        return Ok(false);
    }
    let inserted = sqlx::query(
        r#"
        insert into ask_conversation_memory (
            instance_id, profile_key, chat_id, thread_root_message_id, participant_user_id,
            command_message_id, reply_to_message_id, user_text, assistant_text,
            source_message_ids, source_urls, expires_at
        ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                  now() + ($12::bigint * interval '1 day'))
        on conflict (instance_id, profile_key, chat_id, thread_root_message_id,
                     participant_user_id, command_message_id) do update
          set reply_to_message_id = excluded.reply_to_message_id,
              user_text = excluded.user_text,
              assistant_text = excluded.assistant_text,
              source_message_ids = excluded.source_message_ids,
              source_urls = excluded.source_urls,
              created_at = now(), expires_at = excluded.expires_at
          where ask_conversation_memory.expires_at <= now()
        "#,
    )
    .bind(&scope.instance_id)
    .bind(&scope.profile_key)
    .bind(scope.chat_id)
    .bind(scope.thread_root_message_id)
    .bind(scope.participant_user_id)
    .bind(turn.request_message_id)
    .bind(turn.reply_to_message_id)
    .bind(user_text)
    .bind(assistant_text)
    .bind(Json(&source_message_ids))
    .bind(Json(&source_urls))
    .bind(i64::from(retention_days))
    .execute(&mut *transaction)
    .await?
    .rows_affected()
        > 0;
    sqlx::query(
        r#"
        delete from ask_conversation_memory
        where instance_id = $1 and profile_key = $2 and chat_id = $3 and thread_root_message_id = $4
          and participant_user_id = $5
          and command_message_id in (
            select command_message_id from ask_conversation_memory
            where instance_id = $1 and profile_key = $2 and chat_id = $3 and thread_root_message_id = $4
              and participant_user_id = $5
            order by created_at desc, command_message_id desc
            offset $6
          )
        "#,
    )
    .bind(&scope.instance_id)
    .bind(&scope.profile_key)
    .bind(scope.chat_id)
    .bind(scope.thread_root_message_id)
    .bind(scope.participant_user_id)
    .bind(i64::try_from(max_turns).unwrap_or(i64::MAX))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(inserted)
}

/// Explicitly forgets one participant's history in one verified chat/thread.
pub async fn forget_conversation(pool: &PgPool, scope: &ConversationScope) -> anyhow::Result<u64> {
    let mut transaction = pool.begin().await?;
    lock_scope(&mut transaction, scope).await?;
    sqlx::query(
        r#"
        insert into ask_conversation_memory_epochs (
            instance_id, profile_key, chat_id, thread_root_message_id, participant_user_id, epoch
        ) values ($1, $2, $3, $4, $5, 1)
        on conflict (instance_id, profile_key, chat_id, thread_root_message_id, participant_user_id)
        do update set epoch = ask_conversation_memory_epochs.epoch + 1, updated_at = now()
        "#,
    )
    .bind(&scope.instance_id)
    .bind(&scope.profile_key)
    .bind(scope.chat_id)
    .bind(scope.thread_root_message_id)
    .bind(scope.participant_user_id)
    .execute(&mut *transaction)
    .await?;
    let result = sqlx::query(
        r#"
        delete from ask_conversation_memory
        where instance_id = $1 and profile_key = $2 and chat_id = $3 and thread_root_message_id = $4
          and participant_user_id = $5
        "#,
    )
    .bind(&scope.instance_id)
    .bind(&scope.profile_key)
    .bind(scope.chat_id)
    .bind(scope.thread_root_message_id)
    .bind(scope.participant_user_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(result.rows_affected())
}

pub async fn capture_epoch(pool: &PgPool, scope: &ConversationScope) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        r#"
        select coalesce((
            select epoch from ask_conversation_memory_epochs
            where instance_id = $1 and profile_key = $2 and chat_id = $3
              and thread_root_message_id = $4 and participant_user_id = $5
        ), 0)
        "#,
    )
    .bind(&scope.instance_id)
    .bind(&scope.profile_key)
    .bind(scope.chat_id)
    .bind(scope.thread_root_message_id)
    .bind(scope.participant_user_id)
    .fetch_one(pool)
    .await?)
}

async fn read_epoch_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &ConversationScope,
) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        r#"
        select coalesce((
            select epoch from ask_conversation_memory_epochs
            where instance_id = $1 and profile_key = $2 and chat_id = $3
              and thread_root_message_id = $4 and participant_user_id = $5
        ), 0)
        "#,
    )
    .bind(&scope.instance_id)
    .bind(&scope.profile_key)
    .bind(scope.chat_id)
    .bind(scope.thread_root_message_id)
    .bind(scope.participant_user_id)
    .fetch_one(&mut **transaction)
    .await?)
}

/// Removes expired records across idle scopes, not just scopes receiving new turns.
pub async fn purge_expired(pool: &PgPool) -> anyhow::Result<u64> {
    let mut transaction = pool.begin().await?;
    let expired_turns =
        sqlx::query("delete from ask_conversation_memory where expires_at <= now()")
            .execute(&mut *transaction)
            .await?
            .rows_affected();
    // Agent runs have a much shorter hard deadline; a week-old epoch cannot
    // belong to a live request and can be discarded to bound tombstone growth.
    let expired_epochs = sqlx::query(
        "delete from ask_conversation_memory_epochs where updated_at < now() - interval '7 days'",
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    transaction.commit().await?;
    Ok(expired_turns.saturating_add(expired_epochs))
}

async fn lock_scope(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &ConversationScope,
) -> anyhow::Result<()> {
    sqlx::query(
        "select pg_advisory_xact_lock(hashtextextended($1 || ':' || $2 || ':' || $3::text || ':' || $4::text || ':' || $5::text, 0))",
    )
    .bind(&scope.instance_id)
    .bind(&scope.profile_key)
    .bind(scope.chat_id)
    .bind(scope.thread_root_message_id)
    .bind(scope.participant_user_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub fn bound_turns(
    turns: Vec<ConversationTurn>,
    max_context_chars: usize,
) -> Vec<ConversationTurn> {
    let mut selected = Vec::new();
    let mut used_chars = 0usize;
    for mut turn in turns.into_iter().rev() {
        let needed = turn.user_text.chars().count() + turn.assistant_text.chars().count();
        let remaining = max_context_chars.saturating_sub(used_chars);
        if needed <= remaining {
            used_chars += needed;
            selected.push(turn);
            continue;
        }
        if selected.is_empty() && remaining > 0 {
            let question_budget = remaining / 3;
            turn.user_text = truncate_text(&turn.user_text, question_budget);
            turn.assistant_text = truncate_text(
                &turn.assistant_text,
                remaining.saturating_sub(turn.user_text.chars().count()),
            );
            selected.push(turn);
        }
        break;
    }
    selected.reverse();
    selected
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    if max_chars == 0 {
        return String::new();
    }
    let mut shortened = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    shortened.push('…');
    shortened
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_history_keeps_newest_whole_turns_and_caps_a_large_latest_turn() {
        let turns = vec![
            turn(1, "old question", "old answer"),
            turn(2, "q".repeat(40), "a".repeat(60)),
        ];
        let bounded = bound_turns(turns, 32);
        assert_eq!(bounded.len(), 1);
        assert_eq!(bounded[0].request_message_id, 2);
        assert!(
            bounded[0].user_text.chars().count() + bounded[0].assistant_text.chars().count() <= 32
        );
    }

    #[test]
    fn memory_turn_retains_message_provenance() {
        let turn = ConversationTurn {
            request_message_id: 40,
            reply_to_message_id: Some(39),
            user_text: "question".to_owned(),
            assistant_text: "answer".to_owned(),
            source_message_ids: vec![8, 9],
            source_urls: vec!["https://example.invalid/source".to_owned()],
        };
        let json = serde_json::to_value(&turn).unwrap();
        assert_eq!(json["source_message_ids"], serde_json::json!([8, 9]));
        assert_eq!(json["source_urls"].as_array().unwrap().len(), 1);
    }

    fn turn(
        id: i32,
        user_text: impl Into<String>,
        assistant_text: impl Into<String>,
    ) -> ConversationTurn {
        ConversationTurn {
            request_message_id: id,
            reply_to_message_id: None,
            user_text: user_text.into(),
            assistant_text: assistant_text.into(),
            source_message_ids: Vec::new(),
            source_urls: Vec::new(),
        }
    }
}
