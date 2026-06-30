use async_trait::async_trait;

#[derive(Clone, Copy)]
pub struct LlmRequest<'a> {
    pub model: &'a str,
    pub prompt: &'a str,
    pub image_base64: Option<&'a str>,
    pub temperature: f32,
    pub num_predict: u32,
}

pub struct LlmResponse {
    pub content: String,
}

pub struct GeneratedText {
    pub provider: String,
    pub model: String,
    pub content: String,
    pub image_used: bool,
}

#[async_trait]
pub trait LlmClient {
    async fn generate(&self, request: LlmRequest<'_>) -> anyhow::Result<LlmResponse>;
}
