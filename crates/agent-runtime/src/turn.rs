//! Лимиты витка (§6 спеки): защита от циклической модели и сломанного MCP.

use serde::{Deserialize, Serialize};

/// Жёсткие рамки одного витка агента.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TurnLimits {
    /// Сколько раз модель вызывается за виток.
    pub max_model_roundtrips: u32,
    /// Сколько tool-вызовов разрешено за виток.
    pub max_tool_calls: u32,
    /// Стенное время витка в секундах.
    pub max_wall_time_secs: u64,
}
