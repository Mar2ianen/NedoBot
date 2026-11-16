use serde_json::Value;
use teloxide::utils::rich_text::RenderedMessage;

use crate::features::ask::conversation_memory::{ConversationScope, ConversationTurn};

#[derive(Clone, Debug)]
pub struct AskSandboxFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AskProgress {
    Preparing,
    ResolvingPerson,
    SearchingChat,
    CheckingExternalSources,
    CheckingNotes,
    Computing,
    FormingAnswer,
}

pub struct AskCommandInput {
    pub chat_id: i64,
    pub scope_chat_id: i64,
    pub command_message_id: i32,
    pub requester_user_id: i64,
    pub requester_identity: String,
    pub question: String,
    pub reply_to_message_id: Option<i32>,
    pub reply_context: Option<String>,
    pub reply_image_base64: Option<String>,
    pub sandbox_files: Vec<AskSandboxFile>,
    /// Production `/ask` may save verified notes; diagnostic replay remains read-only.
    pub allow_mutations: bool,
}

pub struct AskAnswer {
    pub markdown: String,
    pub rendered: RenderedMessage,
    pub ask_run_id: Option<i64>,
    pub pending_conversation_memory: Option<PendingConversationMemory>,
}

pub struct PendingConversationMemory {
    pub scope: ConversationScope,
    pub turn: ConversationTurn,
    pub retention_days: u32,
    pub max_turns: usize,
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
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<AskFailureTag>().map(|tag| tag.kind))
            .unwrap_or(Self::GenerationError)
    }
}

#[derive(Debug)]
pub struct AskFailureTag {
    pub kind: AskFailureKind,
    source: anyhow::Error,
}

impl AskFailureTag {
    pub fn wrap(kind: AskFailureKind, source: anyhow::Error) -> anyhow::Error {
        anyhow::Error::new(Self { kind, source })
    }
}

impl std::fmt::Display for AskFailureTag {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(formatter)
    }
}

impl std::error::Error for AskFailureTag {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
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

#[cfg(test)]
mod tests {
    use super::{AskFailureKind, AskFailureTag};

    #[test]
    fn ask_failure_categories_come_from_typed_error_chain_tags() {
        for kind in [
            AskFailureKind::Timeout,
            AskFailureKind::ToolError,
            AskFailureKind::InvalidAction,
        ] {
            let error = AskFailureTag::wrap(kind, anyhow::anyhow!("private provider detail"))
                .context("agent request failed");
            assert_eq!(AskFailureKind::from_error(&error), kind);
        }
    }

    #[test]
    fn untagged_error_text_does_not_change_failure_category() {
        for message in [
            "timed out",
            "MCP database unavailable",
            "invalid action: no final answer",
        ] {
            let error = anyhow::anyhow!(message);
            assert_eq!(
                AskFailureKind::from_error(&error),
                AskFailureKind::GenerationError
            );
        }
    }
}
