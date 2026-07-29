use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::Config;
use crate::http;
use crate::llm::profiles::StructuredOutputMode;
use crate::llm::types::{LlmClient, LlmRequest, LlmResponse, LlmTransportError};

pub struct OllamaClient<'a> {
    base_url: &'a str,
    api_key: &'a str,
    timeout: Duration,
    structured_output_mode: StructuredOutputMode,
}

impl<'a> OllamaClient<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self::with_profile(
            &config.ollama_base_url,
            &config.ollama_api_key,
            Duration::from_secs(60),
            StructuredOutputMode::JsonObject,
        )
    }

    pub fn with_profile(
        base_url: &'a str,
        api_key: &'a str,
        timeout: Duration,
        structured_output_mode: StructuredOutputMode,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/'),
            api_key: api_key.trim(),
            timeout,
            structured_output_mode,
        }
    }
}

#[async_trait]
impl LlmClient for OllamaClient<'_> {
    async fn generate(&self, request: LlmRequest<'_>) -> anyhow::Result<LlmResponse> {
        let images = request.image_base64.into_iter().collect::<Vec<_>>();
        let mut messages = Vec::new();
        if let Some(system_prompt) = request.system_prompt {
            messages.push(OllamaMessage {
                role: "system",
                content: system_prompt,
                images: Vec::new(),
            });
        }
        messages.push(OllamaMessage {
            role: "user",
            content: request.prompt,
            images,
        });

        let body = OllamaChatRequest {
            model: request.model,
            messages,
            stream: false,
            options: OllamaOptions {
                temperature: request.temperature,
                num_predict: request.num_predict,
            },
            // Ollama Cloud currently honors JSON mode for Gemma, but can ignore a
            // schema object and return fenced/incomplete JSON. The typed validator
            // still enforces the requested schema after generation.
            format: output_format(
                request.structured_output.is_some(),
                self.structured_output_mode,
            ),
        };

        let response = http::client(self.timeout)?
            .post(format!("{}/api/chat", self.base_url))
            .bearer_auth(self.api_key)
            .json(&body)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(LlmTransportError::http_status(response.status().as_u16()).into());
        }
        let response = response.json::<OllamaChatResponse>().await?;

        if let Some(error) = response.error {
            anyhow::bail!(error);
        }

        let content = response
            .message
            .map(|message| message.content)
            .unwrap_or_default();

        if content.trim().is_empty() {
            return Err(LlmTransportError::empty_response().into());
        }

        Ok(LlmResponse { content })
    }
}

fn output_format(
    has_structured_output: bool,
    structured_output_mode: StructuredOutputMode,
) -> Option<&'static str> {
    (has_structured_output && structured_output_mode != StructuredOutputMode::PromptOnly)
        .then_some("json")
}

#[derive(Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaMessage<'a>>,
    stream: bool,
    options: OllamaOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'static str>,
}

#[derive(Serialize)]
struct OllamaMessage<'a> {
    role: &'a str,
    content: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<&'a str>,
}

#[derive(Serialize)]
struct OllamaOptions {
    temperature: f32,
    num_predict: u32,
}

#[derive(Deserialize)]
struct OllamaChatResponse {
    message: Option<OllamaResponseMessage>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct OllamaResponseMessage {
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structured_output_uses_ollama_json_mode() {
        let request = OllamaChatRequest {
            model: "gemma",
            messages: Vec::new(),
            stream: false,
            options: OllamaOptions {
                temperature: 0.4,
                num_predict: 90,
            },
            format: Some("json"),
        };

        assert_eq!(serde_json::to_value(request).unwrap()["format"], "json");
    }

    #[test]
    fn prompt_only_profile_omits_ollama_json_mode() {
        assert_eq!(output_format(true, StructuredOutputMode::PromptOnly), None);
        assert_eq!(
            output_format(true, StructuredOutputMode::JsonSchema),
            Some("json")
        );
    }
}
