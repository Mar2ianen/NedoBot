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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationFailureReason {
    EmptyOutput,
    FinalPunctuation,
    MissingChatLink,
    MultipleChatLinks,
    MalformedChatLink,
    UnsupportedChatLinkLabel,
    TooLong,
    TooShort,
    TooManySentences,
    TooLittleCyrillic,
    RawLink,
    GenericCta,
    InventedChatActivity,
    VictimTone,
    NumberWord,
    NoSubstantiveTopicWord,
    MostlyEnglish,
    InvalidJson,
    InvalidMetadata,
    ChatEvidence,
    SearchProvenance,
    SourceLink,
    BlockedTerm,
    Unknown,
}

impl ValidationFailureReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmptyOutput => "empty_output",
            Self::FinalPunctuation => "final_punctuation",
            Self::MissingChatLink => "missing_chat_link",
            Self::MultipleChatLinks => "multiple_chat_links",
            Self::MalformedChatLink => "malformed_chat_link",
            Self::UnsupportedChatLinkLabel => "unsupported_chat_link_label",
            Self::TooLong => "too_long",
            Self::TooShort => "too_short",
            Self::TooManySentences => "too_many_sentences",
            Self::TooLittleCyrillic => "too_little_cyrillic",
            Self::RawLink => "raw_link",
            Self::GenericCta => "generic_cta",
            Self::InventedChatActivity => "invented_chat_activity",
            Self::VictimTone => "victim_tone",
            Self::NumberWord => "number_word",
            Self::NoSubstantiveTopicWord => "no_substantive_topic_word",
            Self::MostlyEnglish => "mostly_english",
            Self::InvalidJson => "invalid_json",
            Self::InvalidMetadata => "invalid_metadata",
            Self::ChatEvidence => "chat_evidence",
            Self::SearchProvenance => "search_provenance",
            Self::SourceLink => "source_link",
            Self::BlockedTerm => "blocked_term",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for ValidationFailureReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ValidationFailure {
    pub reason: ValidationFailureReason,
}

impl ValidationFailure {
    pub const fn new(reason: ValidationFailureReason) -> Self {
        Self { reason }
    }
}

pub fn validation_error(reason: ValidationFailureReason) -> anyhow::Error {
    anyhow::Error::new(ValidationFailure::new(reason))
}

impl fmt::Display for ValidationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "LLM output validation failed: {}", self.reason)
    }
}

impl std::error::Error for ValidationFailure {}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_reason: Option<ValidationFailureReason>,
}

pub struct GeneratedText {
    pub provider: String,
    pub model: String,
    pub content: String,
    pub image_used: bool,
    pub attempts: Vec<LlmAttempt>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_validation_reason_as_safe_stable_code() {
        let attempts = vec![LlmAttempt {
            provider: "gemini".to_string(),
            model: "test-model".to_string(),
            outcome: "validation_failed".to_string(),
            validation_reason: Some(ValidationFailureReason::MissingChatLink),
        }];

        let value = serde_json::to_value(attempts).unwrap();
        assert_eq!(value[0]["validation_reason"], "missing_chat_link");
        assert!(!value.to_string().contains("test response"));
    }
}
