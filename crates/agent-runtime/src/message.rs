//! Канонический внутренний IR сообщений.
//!
//! История сессии хранится в этих типах. Проекции в model API
//! (`genai`), MCP и ACP строятся конвертерами снаружи крейта.

use serde::{Deserialize, Serialize};

/// Роль автора сообщения во внутренней истории.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    System,
    Developer,
    User,
    Assistant,
}

/// Идентификатор сообщения внутри сессии.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

/// Метаданные сообщения: происхождение и поколение компактификации.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageMeta {
    /// Поколение истории (0 — сырая, 1+ — после compaction).
    pub generation: u32,
    /// Произвольная пометка источника (например, `ask`, ` compaction`).
    pub origin: Option<String>,
}

/// Сообщение внутренней истории.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub role: Role,
    pub content: Vec<Content>,
    pub meta: MessageMeta,
}

/// Единица содержимого сообщения.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Content {
    Text(String),
    Image(ImageRef),
    File(FileRef),
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    Reasoning(String),
}

/// Ссылка на изображение (байты хранит владелец артефакта, не IR).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageRef {
    pub mime_type: String,
    pub digest: Option<String>,
}

/// Ссылка на файл/артефакт.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRef {
    pub path: Option<String>,
    pub digest: Option<String>,
    pub mime_type: Option<String>,
}

/// Вызов инструмента моделью.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Результат вызова инструмента.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub content: Vec<Content>,
    pub status: ToolStatus,
}

/// Статус результата инструмента.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolStatus {
    Completed,
    Failed,
    Denied,
    SkippedDuplicate,
}

impl Message {
    /// Создаёт текстовое сообщение с поколением 0.
    pub fn text(id: impl Into<String>, role: Role, text: impl Into<String>) -> Self {
        Self {
            id: MessageId(id.into()),
            role,
            content: vec![Content::Text(text.into())],
            meta: MessageMeta {
                generation: 0,
                origin: None,
            },
        }
    }
}
