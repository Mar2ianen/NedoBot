use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, QueryBuilder, Row};

#[derive(Clone)]
pub struct SpamReputationStore {
    path: PathBuf,
    lock_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncSummary {
    pub spammer_count: usize,
    pub source_decision_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Ledger {
    version: u8,
    decisions: Vec<Decision>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Decision {
    instance_id: String,
    telegram_user_id: i64,
    is_spammer: bool,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            version: 1,
            decisions: Vec::new(),
        }
    }
}

impl SpamReputationStore {
    pub async fn connect(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_owned();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("create spam reputation directory {}", parent.display())
            })?;
        }
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".lock");
        let store = Self {
            path,
            lock_path: PathBuf::from(lock_path),
        };
        let path_for_init = store.path.clone();
        let lock_for_init = store.lock_path.clone();
        tokio::task::spawn_blocking(move || {
            with_ledger(&path_for_init, &lock_for_init, true, |_| Ok(()))
        })
        .await??;
        Ok(store)
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
        let path = self.path.clone();
        let lock_path = self.lock_path.clone();
        let instance_id = instance_id.to_owned();
        let local_decisions = local_decisions.to_vec();
        tokio::task::spawn_blocking(move || {
            with_ledger(&path, &lock_path, true, |ledger| {
                let previous_active = ledger
                    .decisions
                    .iter()
                    .filter(|decision| decision.instance_id == instance_id && decision.is_spammer)
                    .map(|decision| decision.telegram_user_id)
                    .collect::<HashSet<_>>();

                for (telegram_user_id, is_spammer) in local_decisions {
                    let existing = ledger.decisions.iter_mut().find(|decision| {
                        decision.instance_id == instance_id
                            && decision.telegram_user_id == telegram_user_id
                    });
                    if is_spammer {
                        match existing {
                            Some(decision) => decision.is_spammer = true,
                            None => ledger.decisions.push(Decision {
                                instance_id: instance_id.clone(),
                                telegram_user_id,
                                is_spammer: true,
                            }),
                        }
                    } else if previous_active.contains(&telegram_user_id)
                        && let Some(decision) = existing
                    {
                        decision.is_spammer = false;
                    }
                }
                ledger.decisions.sort_by(|left, right| {
                    (&left.instance_id, left.telegram_user_id)
                        .cmp(&(&right.instance_id, right.telegram_user_id))
                });
                Ok(())
            })
        })
        .await??;
        Ok(())
    }

    async fn active_decisions(&self) -> anyhow::Result<Vec<(i64, String)>> {
        let path = self.path.clone();
        let lock_path = self.lock_path.clone();
        tokio::task::spawn_blocking(move || {
            with_ledger(&path, &lock_path, false, |ledger| {
                Ok(ledger
                    .decisions
                    .iter()
                    .filter(|decision| decision.is_spammer)
                    .map(|decision| (decision.telegram_user_id, decision.instance_id.clone()))
                    .collect())
            })
        })
        .await?
    }
}

fn with_ledger<T>(
    path: &Path,
    lock_path: &Path,
    write: bool,
    operation: impl FnOnce(&mut Ledger) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .with_context(|| format!("open spam reputation lock {}", lock_path.display()))?;
    if write {
        FileExt::lock_exclusive(&lock)?;
    } else {
        FileExt::lock_shared(&lock)?;
    }
    set_private_permissions(lock_path)?;

    let mut ledger = match fs::read(path) {
        Ok(bytes) if bytes.starts_with(b"SQLite format 3\0") => bail!(
            "{} is a legacy SQLite reputation database; export it with scripts/export_spam_reputation.py before starting this version",
            path.display()
        ),
        Ok(bytes) => serde_json::from_slice::<Ledger>(&bytes)
            .with_context(|| format!("parse spam reputation ledger {}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ledger::default(),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read spam reputation ledger {}", path.display()));
        }
    };
    anyhow::ensure!(
        ledger.version == 1,
        "unsupported spam reputation ledger version"
    );
    let before = ledger.clone();
    let result = operation(&mut ledger)?;
    if write && ledger != before {
        ledger.decisions.sort_by(|left, right| {
            (&left.instance_id, left.telegram_user_id)
                .cmp(&(&right.instance_id, right.telegram_user_id))
        });
        write_ledger_atomically(path, &ledger)?;
    } else if write && !path.exists() {
        write_ledger_atomically(path, &ledger)?;
    }
    FileExt::unlock(&lock)?;
    Ok(result)
}

fn write_ledger_atomically(path: &Path, ledger: &Ledger) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temporary, ledger)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    set_private_permissions(temporary.path())?;
    temporary
        .persist(path)
        .with_context(|| format!("replace spam reputation ledger {}", path.display()))?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> anyhow::Result<()> {
    Ok(())
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
    use std::fs;

    use tempfile::tempdir;

    use super::SpamReputationStore;

    #[tokio::test]
    async fn unions_sources_and_retracts_only_the_source_that_changed() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("shared.json");
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
    async fn polling_without_changes_is_idempotent_and_file_is_private() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("shared.json");
        let store = SpamReputationStore::connect(&path).await.unwrap();

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
        assert!(fs::read_to_string(path).unwrap().contains("\"version\":1"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&store.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn concurrent_instance_writers_keep_both_decision_sets() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("shared.json");
        let nedonews = SpamReputationStore::connect(&path).await.unwrap();
        let pvo = SpamReputationStore::connect(&path).await.unwrap();

        let (first, second) = tokio::join!(
            nedonews.reconcile_instance_decisions("nedonews", &[(42, true), (43, true)]),
            pvo.reconcile_instance_decisions("pvo", &[(42, true), (44, true)])
        );
        first.unwrap();
        second.unwrap();

        let mut decisions = nedonews.active_decisions().await.unwrap();
        decisions.sort();
        assert_eq!(
            decisions,
            vec![
                (42, "nedonews".to_string()),
                (42, "pvo".to_string()),
                (43, "nedonews".to_string()),
                (44, "pvo".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn rejects_legacy_sqlite_file_instead_of_overwriting_it() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("legacy.sqlite");
        fs::write(&path, b"SQLite format 3\0legacy data").unwrap();
        let error = match SpamReputationStore::connect(&path).await {
            Ok(_) => panic!("legacy database must not be overwritten"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("legacy SQLite"));
        assert!(fs::read(&path).unwrap().starts_with(b"SQLite format 3\0"));
    }
}
