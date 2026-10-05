//! Отмена витка (§20 спеки).
//!
//! Один токен на виток, дочерние — на model request, tool-вызовы
//! и compaction. Отмена идёт вниз и никогда не удаляет уже
//! зафиксированные side effects: journal различает requested,
//! started, committed и received.

use crate::turn::TurnLimits;

pub use tokio_util::sync::CancellationToken;

/// Контекст витка: лимиты + отмена.
#[derive(Debug, Clone)]
pub struct TurnContext {
    pub turn_id: String,
    pub limits: TurnLimits,
    pub cancel: CancellationToken,
}

impl TurnContext {
    /// Создаёт корневой контекст витка.
    pub fn new(turn_id: String, limits: TurnLimits) -> Self {
        Self {
            turn_id,
            limits,
            cancel: CancellationToken::new(),
        }
    }

    /// Отмена родителя идёт к потомкам; отмена потомка не затрагивает родителя.
    pub fn child(&self, turn_id: String) -> Self {
        Self {
            turn_id,
            limits: self.limits,
            cancel: self.cancel.child_token(),
        }
    }

    /// True, если запрошена отмена.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelling_child_does_not_cancel_parent_or_sibling() {
        let parent = TurnContext::new(
            "parent".into(),
            TurnLimits {
                max_model_roundtrips: 3,
                max_tool_calls: 3,
                max_wall_time_secs: 10,
            },
        );
        let child = parent.child("child".into());
        let sibling = parent.child("sibling".into());
        child.cancel.cancel();
        assert!(child.is_cancelled());
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());
        parent.cancel.cancel();
        assert!(sibling.is_cancelled());
    }
}
