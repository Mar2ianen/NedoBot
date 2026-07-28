use async_trait::async_trait;
use std::fmt;

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmTransportError {
    Configuration,
    EmptyResponse,
    HttpStatus(u16),
}

impl LlmTransportError {
    pub fn configuration() -> Self {
        Self::Configuration
    }

    pub fn http_status(status: u16) -> Self {
        Self::HttpStatus(status)
    }

    pub fn empty_response() -> Self {
        Self::EmptyResponse
    }
}

impl fmt::Display for LlmTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration => formatter.write_str("LLM transport configuration is invalid"),
            Self::EmptyResponse => formatter.write_str("LLM returned an empty response"),
            Self::HttpStatus(status) => {
                write!(
                    formatter,
                    "LLM transport request failed with HTTP status {status}"
                )
            }
        }
    }
}

impl std::error::Error for LlmTransportError {}

#[derive(Clone, Copy)]
pub struct StructuredOutput<'a> {
    pub name: &'a str,
    pub schema: &'a Value,
}

#[derive(Clone, Copy)]
pub struct LlmRequest<'a> {
    pub model: &'a str,
    pub system_prompt: Option<&'a str>,
    pub prompt: &'a str,
    pub image_base64: Option<&'a str>,
    pub temperature: f32,
    pub num_predict: u32,
    pub structured_output: Option<StructuredOutput<'a>>,
}

pub struct LlmResponse {
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmAttempt {
    pub provider: String,
    pub model: String,
    pub outcome: String,
}

pub struct GeneratedText {
    pub provider: String,
    pub model: String,
    pub content: String,
    pub image_used: bool,
    pub attempts: Vec<LlmAttempt>,
}

#[async_trait]
pub trait LlmClient {
    async fn generate(&self, request: LlmRequest<'_>) -> anyhow::Result<LlmResponse>;
}
