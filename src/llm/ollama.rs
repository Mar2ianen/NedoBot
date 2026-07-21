use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::Config;
use crate::http;
use crate::llm::types::{LlmClient, LlmRequest, LlmResponse};

pub struct OllamaClient<'a> {
    config: &'a Config,
}

impl<'a> OllamaClient<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self { config }
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
            format: request.structured_output.map(|_| "json"),
        };

        let response = http::client(Duration::from_secs(60))?
            .post(format!(
                "{}/api/chat",
                self.config.ollama_base_url.trim_end_matches('/')
            ))
            .bearer_auth(&self.config.ollama_api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json::<OllamaChatResponse>()
            .await?;

        if let Some(error) = response.error {
            anyhow::bail!(error);
        }

        let content = response
            .message
            .map(|message| message.content)
            .unwrap_or_default();

        if content.trim().is_empty() {
            anyhow::bail!("empty Ollama response");
        }

        Ok(LlmResponse { content })
    }
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
}
