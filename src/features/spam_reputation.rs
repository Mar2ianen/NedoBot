use std::{collections::HashSet, path::Path, time::Duration};

use anyhow::Context;
use sqlx::{PgPool, QueryBuilder, Row, SqlitePool, sqlite::SqlitePoolOptions};

#[derive(Clone)]
pub struct SpamReputationStore {
    pool: SqlitePool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncSummary {
    pub spammer_count: usize,
    pub source_decision_count: usize,
}

impl SpamReputationStore {
    pub async fn connect(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("create spam reputation directory {}", parent.display())
            })?;
        }

        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(10))
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .with_context(|| format!("open shared spam reputation database {}", path.display()))?;

        sqlx::query(
            "create table if not exists spammer_decisions (
                instance_id text not null,
                telegram_user_id integer not null,
                is_spammer integer not null check (is_spammer in (0, 1)),
                updated_at integer not null default (cast(strftime('%s', 'now') as integer)),
                primary key (instance_id, telegram_user_id)
            )",
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "create index if not exists spammer_decisions_active_idx
             on spammer_decisions (telegram_user_id, instance_id) where is_spammer = 1",
        )
        .execute(&pool)
        .await?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(0o600);
            tokio::fs::set_permissions(path, permissions)
                .await
                .with_context(|| {
                    format!(
                        "restrict permissions on spam reputation database {}",
                        path.display()
                    )
                })?;
        }

        Ok(Self { pool })
    }

    pub async fn synchronize_instance(
        &self,
        pg: &PgPool,
        instance_id: &str,
        chat_ids: &[i64],
    ) -> anyhow::Result<SyncSummary> {
        anyhow::ensure!(
            !instance_id.trim().is_empty(),
            "instance id must not be empty"
        );
        anyhow::ensure!(
            !chat_ids.is_empty(),
            "at least one managed chat is required"
        );

        let local_decisions = sqlx::query(
            "select telegram_user_id, bool_or(is_spammer) as is_spammer
             from telegram_chat_users
             where chat_id = any($1::bigint[])
             group by telegram_user_id",
        )
        .bind(chat_ids)
        .fetch_all(pg)
        .await?
        .into_iter()
        .map(|row| {
            (
                row.get::<i64, _>("telegram_user_id"),
                row.get::<bool, _>("is_spammer"),
            )
        })
        .collect::<Vec<_>>();

        self.reconcile_instance_decisions(instance_id, &local_decisions)
            .await?;
        let active_decisions = self.active_decisions().await?;
        replace_postgres_snapshot(pg, &active_decisions).await?;

        let spammer_count = active_decisions
            .iter()
            .map(|(telegram_user_id, _)| telegram_user_id)
            .collect::<HashSet<_>>()
            .len();
        Ok(SyncSummary {
            spammer_count,
            source_decision_count: active_decisions.len(),
        })
    }

    async fn reconcile_instance_decisions(
        &self,
        instance_id: &str,
        local_decisions: &[(i64, bool)],
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        let previously_active = sqlx::query_scalar::<_, i64>(
            "select telegram_user_id from spammer_decisions
             where instance_id = ? and is_spammer = 1",
        )
        .bind(instance_id)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect::<HashSet<_>>();

        for (telegram_user_id, is_spammer) in local_decisions {
            if *is_spammer {
                sqlx::query(
                    "insert into spammer_decisions (instance_id, telegram_user_id, is_spammer)
                     values (?, ?, 1)
                     on conflict (instance_id, telegram_user_id) do update set
                         is_spammer = 1,
                         updated_at = cast(strftime('%s', 'now') as integer)
                     where spammer_decisions.is_spammer <> 1",
                )
                .bind(instance_id)
                .bind(telegram_user_id)
                .execute(&mut *tx)
                .await?;
            } else if previously_active.contains(telegram_user_id) {
                sqlx::query(
                    "update spammer_decisions
                     set is_spammer = 0, updated_at = cast(strftime('%s', 'now') as integer)
                     where instance_id = ? and telegram_user_id = ? and is_spammer = 1",
                )
                .bind(instance_id)
                .bind(telegram_user_id)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    async fn active_decisions(&self) -> anyhow::Result<Vec<(i64, String)>> {
        let rows = sqlx::query(
            "select telegram_user_id, instance_id from spammer_decisions
             where is_spammer = 1 order by telegram_user_id, instance_id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| (row.get("telegram_user_id"), row.get("instance_id")))
            .collect())
    }
}

async fn replace_postgres_snapshot(
    pg: &PgPool,
    active_decisions: &[(i64, String)],
) -> anyhow::Result<()> {
    let current = sqlx::query(
        "select telegram_user_id, source_instance_id from shared_spam_reputation
         order by telegram_user_id, source_instance_id",
    )
    .fetch_all(pg)
    .await?
    .into_iter()
    .map(|row| (row.get("telegram_user_id"), row.get("source_instance_id")))
    .collect::<Vec<(i64, String)>>();
    if current == active_decisions {
        return Ok(());
    }

    let mut tx = pg.begin().await?;
    sqlx::query("delete from shared_spam_reputation")
        .execute(&mut *tx)
        .await?;

    if !active_decisions.is_empty() {
        let mut query = QueryBuilder::new(
            "insert into shared_spam_reputation
                (telegram_user_id, source_instance_id, synced_at) ",
        );
        query.push_values(
            active_decisions,
            |mut row, (telegram_user_id, instance_id)| {
                row.push_bind(telegram_user_id)
                    .push_bind(instance_id)
                    .push("now()");
            },
        );
        query.build().execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::SpamReputationStore;

    #[tokio::test]
    async fn unions_sources_and_retracts_only_the_source_that_changed() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("shared.sqlite");
        let main = SpamReputationStore::connect(&path).await.unwrap();
        let pvo = SpamReputationStore::connect(&path).await.unwrap();

        main.reconcile_instance_decisions("main", &[(42, true), (99, false)])
            .await
            .unwrap();
        pvo.reconcile_instance_decisions("pvo", &[(42, true)])
            .await
            .unwrap();
        main.reconcile_instance_decisions("main", &[(42, false), (99, false)])
            .await
            .unwrap();

        assert_eq!(
            main.active_decisions().await.unwrap(),
            vec![(42, "pvo".to_string())]
        );

        pvo.reconcile_instance_decisions("pvo", &[(42, false)])
            .await
            .unwrap();
        assert!(main.active_decisions().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn polling_without_changes_is_idempotent() {
        let directory = tempdir().unwrap();
        let store = SpamReputationStore::connect(directory.path().join("shared.sqlite"))
            .await
            .unwrap();

        for _ in 0..3 {
            store
                .reconcile_instance_decisions("main", &[(7, true)])
                .await
                .unwrap();
        }

        assert_eq!(
            store.active_decisions().await.unwrap(),
            vec![(7, "main".to_string())]
        );
    }
}
