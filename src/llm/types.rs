use async_trait::async_trait;

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

#[async_trait]
pub trait LlmClient {
    async fn generate(&self, request: LlmRequest<'_>) -> anyhow::Result<LlmResponse>;
}
