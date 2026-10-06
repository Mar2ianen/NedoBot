//! Генерик runtime агента: IR, контекст, политика, события, ошибки.
//!
//! Крейт намеренно не зависит от `genai`, `rmcp` и ACP SDK.
//! Внешние протоколы подключаются адаптерами снаружи.

pub mod cancellation;
pub mod compaction;
pub mod context;
pub mod error;
pub mod event;
pub mod message;
pub mod policy;
pub mod retained;
pub mod scheduler;
pub mod store;
pub mod summary;
pub mod turn;
pub mod usage;

pub use cancellation::{CancellationToken, TurnContext};
pub use compaction::{
    CompactionAction, CompactionPlan, CompactionPolicy, GreedyCompactionPolicy, TruncateStrategy,
};
pub use context::{
    ApproxTokenizer, ContextBudget, ContextClass, ContextEntry, ContextPressure, ContextSnapshot,
    EstimateConfidence, ObservationWindow, TokenEstimate, Tokenizer, pressure, truncate_chars,
};
pub use error::{AgentError, ContextErrorKind, ModelErrorKind, PermissionErrorKind, ToolErrorKind};
pub use event::AgentEvent;
pub use message::{Content, Message, MessageMeta, Role, ToolCall, ToolResult, ToolStatus};
pub use policy::{Capability, PermissionDecision, ToolDescriptor};
pub use retained::{InMemoryRetainedStore, RetainedFact, RetainedScope, RetainedStore};
pub use scheduler::{ToolConcurrency, ToolTask, schedule_batches};
pub use store::{
    ArtifactRef, CompactionRecord, InMemorySessionStore, SessionStore, TurnRecord, UsageRecord,
};
pub use summary::{ConversationSummary, FileState};
pub use turn::TurnLimits;
pub use usage::{CostEstimate, Usage, UsageSource};
