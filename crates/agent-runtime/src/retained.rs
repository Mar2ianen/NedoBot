//! Долговременное retained-состояние (§13 спеки).
//!
//! Отдельно от transcript: факт переживает compaction исходного
//! сообщения и хранит provenance вместо вечной копии текста.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Область видимости факта.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RetainedScope {
    Project,
    Session(String),
    User(i64),
}

impl RetainedScope {
    /// Ключ для хранилища.
    fn store_key(&self) -> String {
        match self {
            Self::Project => "project".to_owned(),
            Self::Session(id) => format!("session:{id}"),
            Self::User(id) => format!("user:{id}"),
        }
    }
}

/// Долговечный факт: ключ + значение + откуда взят.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetainedFact {
    pub scope: RetainedScope,
    pub key: String,
    pub value: String,
    /// Id сообщения-источника, если факт извлечён из transcript.
    pub provenance: Option<String>,
    /// Поколение истории, в котором факт зафиксирован.
    pub generation: u32,
}

/// Хранилище фактов. Durable backend (SQLite/Postgres) — снаружи.
// clippy::double_must_use здесь ложный: must_use на BoxFuture вешает сам
// async_trait. Убрать allow, когда уйдём с async_trait на RPITIT с явными
// Send-границами или починят взаимодействие макроса с линтом.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait RetainedStore: Send + Sync {
    /// Создать или заменить факт.
    async fn upsert(&self, fact: RetainedFact);
    /// Прочитать один факт.
    async fn get(&self, scope: &RetainedScope, key: &str) -> Option<RetainedFact>;
    /// Все факты области.
    async fn list_scope(&self, scope: &RetainedScope) -> Vec<RetainedFact>;
}

/// In-memory реализация для тестов и локального CLI.
#[derive(Debug, Default)]
pub struct InMemoryRetainedStore {
    facts: Mutex<HashMap<(String, String), RetainedFact>>,
}

#[async_trait]
impl RetainedStore for InMemoryRetainedStore {
    async fn upsert(&self, fact: RetainedFact) {
        let key = (fact.scope.store_key(), fact.key.clone());
        self.facts
            .lock()
            .map(|mut facts| facts.insert(key, fact))
            .ok();
    }

    async fn get(&self, scope: &RetainedScope, key: &str) -> Option<RetainedFact> {
        self.facts
            .lock()
            .ok()
            .and_then(|facts| facts.get(&(scope.store_key(), key.to_owned())).cloned())
    }

    async fn list_scope(&self, scope: &RetainedScope) -> Vec<RetainedFact> {
        let prefix = scope.store_key();
        self.facts
            .lock()
            .map(|facts| {
                facts
                    .iter()
                    .filter(|((scope_key, _), _)| scope_key == &prefix)
                    .map(|(_, fact)| fact.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upsert_and_get_roundtrip() {
        let store = InMemoryRetainedStore::default();
        store
            .upsert(RetainedFact {
                scope: RetainedScope::Project,
                key: "build-command".to_owned(),
                value: "cargo xtask build".to_owned(),
                provenance: Some("msg-14".to_owned()),
                generation: 0,
            })
            .await;
        let fact = store
            .get(&RetainedScope::Project, "build-command")
            .await
            .expect("fact stored");
        assert_eq!(fact.value, "cargo xtask build");
        assert_eq!(fact.provenance.as_deref(), Some("msg-14"));
    }

    #[tokio::test]
    async fn scopes_are_isolated() {
        let store = InMemoryRetainedStore::default();
        store
            .upsert(RetainedFact {
                scope: RetainedScope::User(1),
                key: "k".to_owned(),
                value: "v1".to_owned(),
                provenance: None,
                generation: 0,
            })
            .await;
        assert!(store.get(&RetainedScope::User(2), "k").await.is_none());
        assert_eq!(store.list_scope(&RetainedScope::User(1)).await.len(), 1);
    }
}
