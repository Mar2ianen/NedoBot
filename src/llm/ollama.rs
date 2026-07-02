use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::Config;
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
        let body = OllamaChatRequest {
            model: request.model,
            messages: vec![OllamaMessage {
                role: "user",
                content: request.prompt,
                images,
            }],
            stream: false,
            options: OllamaOptions {
                temperature: request.temperature,
                num_predict: request.num_predict,
            },
        };

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?;
        let response = client
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
