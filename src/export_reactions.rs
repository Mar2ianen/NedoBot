//! CLI-восстановление только отсутствующих счётчиков из снимка Desktop export.
//! Снимок не содержит историю изменений: события реакций не создаём.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use teloxide::types::ReactionCount;

const BATCH_SIZE: usize = 1_000;

#[derive(Debug, Serialize)]
pub struct ReactionSnapshot {
    pub message_id: i32,
    pub reactions: Vec<ReactionCount>,
    pub total_count: i64,
    // При пустом document_id сохраняем количество, но не вымышленную identity.
    pub unidentified_custom_count: i64,
}

impl ReactionSnapshot {
    pub fn new(message_id: i32, reactions: Vec<ReactionCount>) -> anyhow::Result<Self> {
        Self::with_unidentified_custom(message_id, reactions, 0)
    }

    pub fn with_unidentified_custom(
        message_id: i32,
        reactions: Vec<ReactionCount>,
        unidentified_custom_count: i64,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(message_id > 0, "snapshot message_id must be positive");
        anyhow::ensure!(
            unidentified_custom_count >= 0,
            "unknown custom count must not be negative"
        );
        anyhow::ensure!(
            !reactions.is_empty() || unidentified_custom_count > 0,
            "snapshot must contain reactions"
        );
        let mut seen = HashSet::new();
        let mut total_count = unidentified_custom_count;
        for reaction in &reactions {
            anyhow::ensure!(seen.insert(&reaction.r#type), "duplicate reaction identity");
            anyhow::ensure!(reaction.total_count > 0, "reaction count must be positive");
            let count = i64::try_from(reaction.total_count)?;
            total_count = total_count
                .checked_add(count)
                .ok_or_else(|| anyhow::anyhow!("snapshot reaction total overflow"))?;
        }
        Ok(Self {
            message_id,
            reactions,
            total_count,
            unidentified_custom_count,
        })
    }
}

#[derive(Debug, Default, Serialize)]
pub struct BackfillStats {
    pub candidates: i64,
    pub existing: i64,
    pub missing_messages: i64,
    pub inserted: u64,
}

/// Все партии одной транзакцией, conflict-safe: строки, появившиеся во время
/// импорта, тоже не перезаписываем. `observed_at` — время снимка, не сообщения.
pub async fn backfill_missing_counts(
    pool: &PgPool,
    chat_id: i64,
    snapshots: &[ReactionSnapshot],
    observed_at: DateTime<Utc>,
    export_sha256: &str,
    dry_run: bool,
) -> anyhow::Result<BackfillStats> {
    anyhow::ensure!(
        export_sha256.len() == 64 && export_sha256.bytes().all(|c| c.is_ascii_hexdigit()),
        "export SHA256 must contain 64 hex digits"
    );
    let mut ids = HashSet::new();
    for snapshot in snapshots {
        anyhow::ensure!(
            ids.insert(snapshot.message_id),
            "duplicate snapshot message_id"
        );
        let validated = ReactionSnapshot::with_unidentified_custom(
            snapshot.message_id,
            snapshot.reactions.clone(),
            snapshot.unidentified_custom_count,
        )?;
        anyhow::ensure!(
            validated.total_count == snapshot.total_count,
            "snapshot total mismatch"
        );
    }
    let provenance = serde_json::json!({
        "source": "telegram_export_counts_snapshot",
        "export_sha256": export_sha256,
        "observed_at": observed_at,
        "event_time_known": false,
        "mode": "missing_counts_only"
    });
    let mut tx = pool.begin().await?;
    sqlx::query("set local lock_timeout = '5s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("set local statement_timeout = '60s'")
        .execute(&mut *tx)
        .await?;
    let mut stats = BackfillStats::default();
    for batch in snapshots.chunks(BATCH_SIZE) {
        let payload = serde_json::to_value(batch)?;
        let (candidates, existing, missing): (i64, i64, i64) = sqlx::query_as(
            r#"
            select count(*) filter (where m.message_id is not null and rc.message_id is null),
                   count(*) filter (where rc.message_id is not null),
                   count(*) filter (where m.message_id is null)
            from jsonb_to_recordset($2) as s(message_id integer)
            left join telegram_messages m on m.chat_id = $1 and m.message_id = s.message_id
            left join telegram_message_reaction_counts rc on rc.chat_id = $1 and rc.message_id = s.message_id
            "#,
        ).bind(chat_id).bind(&payload).fetch_one(&mut *tx).await?;
        stats.candidates += candidates;
        stats.existing += existing;
        stats.missing_messages += missing;
        if dry_run {
            continue;
        }
        stats.inserted += sqlx::query(
            r#"
            insert into telegram_message_reaction_counts
                (chat_id, message_id, reactions, total_count, raw_json, event_at)
            select $1, s.message_id, s.reactions, s.total_count,
                   $4::jsonb || jsonb_build_object('unidentified_custom_emoji_count', s.unidentified_custom_count), $3
            from jsonb_to_recordset($2) as s(message_id integer, reactions jsonb, total_count bigint, unidentified_custom_count bigint)
            join telegram_messages m on m.chat_id = $1 and m.message_id = s.message_id
            on conflict (chat_id, message_id) do nothing
            "#,
        ).bind(chat_id).bind(&payload).bind(observed_at).bind(&provenance)
            .execute(&mut *tx).await?.rows_affected();
    }
    tx.commit().await?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use teloxide::types::ReactionType;

    fn thumbs(count: u64) -> ReactionCount {
        ReactionCount {
            r#type: ReactionType::Emoji {
                emoji: "👍".into()
            },
            total_count: count,
        }
    }

    #[test]
    fn snapshot_rejects_duplicates_empty_zero_and_overflow() {
        assert!(ReactionSnapshot::new(1, vec![]).is_err());
        assert!(ReactionSnapshot::new(0, vec![thumbs(1)]).is_err());
        assert!(ReactionSnapshot::new(1, vec![thumbs(0)]).is_err());
        assert!(ReactionSnapshot::new(1, vec![thumbs(1), thumbs(2)]).is_err());
        assert!(ReactionSnapshot::new(1, vec![thumbs(u64::MAX)]).is_err());
        let snapshot = ReactionSnapshot::new(1, vec![thumbs(3)]).unwrap();
        assert_eq!(snapshot.total_count, 3);
        assert_eq!(
            serde_json::to_value(snapshot).unwrap()["reactions"][0]["type"]["emoji"],
            "👍"
        );
    }
}
