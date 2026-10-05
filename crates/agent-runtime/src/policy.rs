//! Политика инструментов и разрешений (§17 спеки).
//!
//! Единая модель для native, MCP и ACP: инструмент декларирует
//! требуемые capabilities, policy отвечает Allow/Deny/Ask.
//! ACP permission request — лишь проекция `Ask` наружу.

use serde::{Deserialize, Serialize};

/// Возможность, требуемая инструментом.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    FsRead(String),
    FsWrite(String),
    ProcessSpawn,
    Network(String),
    GitWrite,
    SecretRead(String),
    Arbitrary(String),
}

/// Решение политики по запросу возможности.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionDecision {
    Allow,
    Deny,
    Ask,
}

/// Описание инструмента, достаточное модели и политике.
/// Схемы аргументов — JSON Schema как `serde_json::Value`,
/// чтобы core не зависел от rmcp/schemars-типов.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub arguments_schema: serde_json::Value,
    pub required_capabilities: Vec<Capability>,
}
