use std::fmt;

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmTransportError {
    Configuration,
    Timeout,
    EmptyResponse,
    InvalidResponse,
    HttpStatus(u16),
    UnsupportedFeature,
    StructuredOutputRejected,
}

impl LlmTransportError {
    pub fn configuration() -> Self {
        Self::Configuration
    }

    pub fn http_status(status: u16) -> Self {
        Self::HttpStatus(status)
    }

    pub fn timeout() -> Self {
        Self::Timeout
    }

    pub fn empty_response() -> Self {
        Self::EmptyResponse
    }

    pub fn invalid_response() -> Self {
        Self::InvalidResponse
    }

    pub fn unsupported_feature() -> Self {
        Self::UnsupportedFeature
    }

    pub fn structured_output_rejected() -> Self {
        Self::StructuredOutputRejected
    }
}

impl fmt::Display for LlmTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration => formatter.write_str("LLM transport configuration is invalid"),
            Self::Timeout => formatter.write_str("LLM transport request timed out"),
            Self::EmptyResponse => formatter.write_str("LLM returned an empty response"),
            Self::InvalidResponse => formatter.write_str("LLM returned an invalid response"),
            Self::HttpStatus(status) => {
                write!(
                    formatter,
                    "LLM transport request failed with HTTP status {status}"
                )
            }
            Self::UnsupportedFeature => {
                formatter.write_str("LLM transport does not support the requested feature")
            }
            Self::StructuredOutputRejected => {
                formatter.write_str("LLM transport rejected the structured output request")
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
