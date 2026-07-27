use serde_json::Value;

#[derive(Clone, Copy)]
pub enum ToolCallStatus {
    Completed,
    Failed,
    SkippedDuplicate,
}

impl ToolCallStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::SkippedDuplicate => "skipped_duplicate",
        }
    }
}

pub struct ToolCallAudit<'a> {
    pub ask_run_id: i64,
    pub step_number: i32,
    pub tool_name: &'a str,
    pub arguments: &'a Value,
    pub status: ToolCallStatus,
    pub result_count: Option<i64>,
    pub latency_ms: Option<i64>,
    pub error_kind: Option<&'a str>,
}

pub struct PendingToolCallAudit<'a> {
    step: usize,
    tool_name: &'a str,
    arguments: &'a Value,
    status: ToolCallStatus,
    result_count: Option<i64>,
    latency_ms: Option<i64>,
    error_kind: Option<&'a str>,
}

impl<'a> PendingToolCallAudit<'a> {
    pub fn completed(
        step: usize,
        tool_name: &'a str,
        arguments: &'a Value,
        result_count: Option<i64>,
        latency_ms: Option<i64>,
    ) -> Self {
        Self {
            step,
            tool_name,
            arguments,
            status: ToolCallStatus::Completed,
            result_count,
            latency_ms,
            error_kind: None,
        }
    }

    pub fn failed(
        step: usize,
        tool_name: &'a str,
        arguments: &'a Value,
        latency_ms: Option<i64>,
        error_kind: &'a str,
    ) -> Self {
        Self {
            step,
            tool_name,
            arguments,
            status: ToolCallStatus::Failed,
            result_count: None,
            latency_ms,
            error_kind: Some(error_kind),
        }
    }

    pub fn duplicate(step: usize, tool_name: &'a str, arguments: &'a Value) -> Self {
        Self {
            step,
            tool_name,
            arguments,
            status: ToolCallStatus::SkippedDuplicate,
            result_count: None,
            latency_ms: Some(0),
            error_kind: Some("duplicate"),
        }
    }

    pub(crate) fn into_audit(self, ask_run_id: i64) -> ToolCallAudit<'a> {
        ToolCallAudit {
            ask_run_id,
            step_number: audit_step_number(self.step),
            tool_name: self.tool_name,
            arguments: self.arguments,
            status: self.status,
            result_count: self.result_count,
            latency_ms: self.latency_ms,
            error_kind: self.error_kind,
        }
    }

    pub(crate) fn tool_name(&self) -> &'a str {
        self.tool_name
    }
}

#[derive(Clone, Copy)]
pub enum AskRunStatus {
    Completed,
    Failed,
}

impl AskRunStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

fn audit_step_number(step: usize) -> i32 {
    step.checked_add(1)
        .and_then(|number| i32::try_from(number).ok())
        .unwrap_or(i32::MAX)
}
