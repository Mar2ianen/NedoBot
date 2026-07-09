use async_trait::async_trait;
use serde::Serialize;

#[derive(Clone, Copy)]
pub struct LlmRequest<'a> {
    pub model: &'a str,
    pub system_prompt: Option<&'a str>,
    pub prompt: &'a str,
    pub image_base64: Option<&'a str>,
    pub temperature: f32,
    pub num_predict: u32,
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
