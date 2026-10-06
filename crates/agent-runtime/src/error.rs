//! Таксономия ошибок runtime (§30 спеки).
//!
//! Никакого `anyhow::Error` через публичную границу runtime.

use thiserror::Error;

/// Ошибка runtime агента.
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("model error: {0}")]
    Model(#[from] ModelError),
    #[error("tool error: {0}")]
    Tool(#[from] ToolError),
    #[error("context error: {0}")]
    Context(#[from] ContextError),
    #[error("permission error: {0}")]
    Permission(#[from] PermissionError),
    #[error("cancelled")]
    Cancelled,
}

/// Ошибка модели с видом, понятным planner'у.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct ModelError {
    pub kind: ModelErrorKind,
    pub message: String,
}

/// Вид ошибки модели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelErrorKind {
    Authentication,
    RateLimited,
    QuotaExceeded,
    /// Сигнал planner'у: compact и ровно один retry.
    ContextExceeded,
    InvalidRequest,
    ProviderUnavailable,
    Transport,
}

/// Ошибка инструмента.
#[derive(Debug, Error)]
#[error("{kind:?}: {message}")]
pub struct ToolError {
    pub kind: ToolErrorKind,
    pub message: String,
}

/// Вид ошибки инструмента.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolErrorKind {
    Forbidden,
    InvalidArguments,
    ExecutionFailed,
    Timeout,
}

/// Ошибка контекста.
#[derive(Debug, Error)]
#[error("{kind:?}: {message}")]
pub struct ContextError {
    pub kind: ContextErrorKind,
    pub message: String,
}

/// Вид ошибки контекста.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextErrorKind {
    BudgetExceeded,
    CompactionFailed,
}

/// Ошибка разрешений.
#[derive(Debug, Error)]
#[error("{kind:?}: {message}")]
pub struct PermissionError {
    pub kind: PermissionErrorKind,
    pub message: String,
}

/// Вид ошибки разрешений.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionErrorKind {
    Denied,
    ApprovalTimeout,
}
