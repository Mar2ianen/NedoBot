//! Персистентность сессии (§21 спеки): append-first journal.
//!
//! Compaction никогда не удаляет источник физически: каждая генерация
//! хранится как запись со ссылкой на родительскую. Большие blobs
//! лежат в файлах по хешу, в journal — только ссылки.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::summary::ConversationSummary;
use crate::usage::Usage;

/// Запись одного витка.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub turn_id: String,
    pub session_id: String,
    pub model: String,
    pub ok: bool,
}

/// Использование модели за виток.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    pub turn_id: String,
    pub usage: Usage,
}

/// Запись компактификации: какая генерация из какой получена.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionRecord {
    pub session_id: String,
    pub from_generation: u32,
    pub to_generation: u32,
    pub summary: ConversationSummary,
}

/// Ссылка на blob (большой tool output, файл, артефакт).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// hex sha256 содержимого.
    pub hash: String,
    pub mime: String,
    pub size: u64,
    pub origin: Option<String>,
}

/// Journal сессии. Durable backend (SQLite/Postgres) — снаружи.
// clippy::double_must_use здесь ложный: must_use на BoxFuture вешает сам
// async_trait. Убрать allow, когда уйдём с async_trait на RPITIT с явными
// Send-границами или починят взаимодействие макроса с линтом.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Добавить запись витка.
    async fn append_turn(&self, turn: TurnRecord);
    /// Добавить отчёт использования.
    async fn record_usage(&self, usage: UsageRecord);
    /// Добавить запись компактификации.
    async fn record_compaction(&self, record: CompactionRecord);
    /// Все витки сессии по порядку добавления.
    async fn turns(&self, session_id: &str) -> Vec<TurnRecord>;
}

/// In-memory journal для тестов и локального CLI.
#[derive(Debug, Default)]
pub struct InMemorySessionStore {
    turns: Mutex<Vec<TurnRecord>>,
    usages: Mutex<Vec<UsageRecord>>,
    compactions: Mutex<Vec<CompactionRecord>>,
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn append_turn(&self, turn: TurnRecord) {
        self.turns.lock().map(|mut t| t.push(turn)).ok();
    }

    async fn record_usage(&self, usage: UsageRecord) {
        self.usages.lock().map(|mut u| u.push(usage)).ok();
    }

    async fn record_compaction(&self, record: CompactionRecord) {
        self.compactions.lock().map(|mut c| c.push(record)).ok();
    }

    async fn turns(&self, session_id: &str) -> Vec<TurnRecord> {
        self.turns
            .lock()
            .map(|turns| {
                turns
                    .iter()
                    .filter(|turn| turn.session_id == session_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn journal_keeps_append_order() {
        let store = InMemorySessionStore::default();
        for id in ["t1", "t2"] {
            store
                .append_turn(TurnRecord {
                    turn_id: id.to_owned(),
                    session_id: "s".to_owned(),
                    model: "m".to_owned(),
                    ok: true,
                })
                .await;
        }
        let turns = store.turns("s").await;
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].turn_id, "t1");
        assert!(store.turns("other").await.is_empty());
    }
}
