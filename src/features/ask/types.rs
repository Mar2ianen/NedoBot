use serde_json::Value;
use teloxide::utils::rich_text::RenderedMessage;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AskProgress {
    Preparing,
    ResolvingPerson,
    SearchingChat,
    CheckingExternalSources,
    CheckingNotes,
    FormingAnswer,
}

pub struct AskCommandInput {
    pub chat_id: i64,
    pub command_message_id: i32,
    pub requester_user_id: i64,
    pub requester_identity: String,
    pub question: String,
    pub reply_to_message_id: Option<i32>,
    pub reply_context: Option<String>,
    pub reply_image_base64: Option<String>,
    /// Production `/ask` may save verified notes; diagnostic replay remains read-only.
    pub allow_mutations: bool,
}

pub struct AskAnswer {
    pub markdown: String,
    pub rendered: RenderedMessage,
    pub ask_run_id: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AskFailureKind {
    Timeout,
    ToolError,
    InvalidAction,
    InvalidOutput,
    GenerationError,
}

impl AskFailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::ToolError => "tool_error",
            Self::InvalidAction => "invalid_action",
            Self::InvalidOutput => "render_validation",
            Self::GenerationError => "generation_error",
        }
    }

    pub fn from_error(error: &anyhow::Error) -> Self {
        let error = error.to_string().to_lowercase();
        if error.contains("timed out") || error.contains("deadline exceeded") {
            Self::Timeout
        } else if error.contains("mcp") || error.contains("database") {
            Self::ToolError
        } else if error.contains("invalid action") || error.contains("final answer") {
            Self::InvalidAction
        } else {
            Self::GenerationError
        }
    }
}

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
    DeliveryPending,
    Completed,
    Failed,
}

impl AskRunStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::DeliveryPending => "delivery_pending",
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
