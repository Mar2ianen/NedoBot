//! Единый event stream runtime (§5 спеки).
//!
//! Model-адаптер не вызывает ACP/CLI напрямую: он издаёт события,
//! а проекторы (ACP, CLI, journal) их потребляют.

use serde::{Deserialize, Serialize};

/// Событие runtime агента.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    TurnStarted { turn: String },
    ModelDelta(String),
    ModelUsage { input: u64, output: u64 },
    ModelFinished,
    ToolRequested { name: String, call_id: String },
    ToolFinished { call_id: String, ok: bool },
    ApprovalRequested { call_id: String, tool: String },
    ApprovalResolved { call_id: String, approved: bool },
    ContextPressure(crate::context::ContextPressure),
    MessageCommitted(String),
    TurnFinished { ok: bool },
}
