use async_trait::async_trait;
use reqwest::header::USER_AGENT;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::Config;
use crate::http;
use crate::llm::types::{LlmClient, LlmRequest, LlmResponse};

pub struct OpenAiCompatClient<'a> {
    api_base: &'a str,
    api_key: &'a str,
}

impl<'a> OpenAiCompatClient<'a> {
    pub fn new(api_base: &'a str, api_key: &'a str) -> Self {
        Self { api_base, api_key }
    }

    pub fn from_config(config: &'a Config) -> Self {
        Self::new(
            config.openai_compat_base_url.trim_end_matches('/'),
            config.openai_compat_api_key.trim(),
        )
    }
}

#[async_trait]
impl LlmClient for OpenAiCompatClient<'_> {
    async fn generate(&self, request: LlmRequest<'_>) -> anyhow::Result<LlmResponse> {
        if self.api_key.is_empty() {
            anyhow::bail!("OpenAI-compatible API key is empty");
        }

        let body = ChatCompletionRequest {
            model: request.model,
            messages: vec![ChatMessage {
                role: "user",
                content: user_content(request.prompt, request.image_base64),
            }],
            temperature: request.temperature,
            max_completion_tokens: request.num_predict,
        };

        let response = http::client(Duration::from_secs(45))?
            .post(format!(
                "{}/chat/completions",
                self.api_base.trim_end_matches('/')
            ))
            .header(USER_AGENT, "tg-ai-bot-teloxide/0.1")
            .bearer_auth(self.api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json::<ChatCompletionResponse>()
            .await?;

        let content = response
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content)
            .unwrap_or_default();

        if content.trim().is_empty() {
            anyhow::bail!("empty OpenAI-compatible response");
        }

        Ok(LlmResponse { content })
    }
}

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
    max_completion_tokens: u32,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: MessageContent<'a>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum MessageContent<'a> {
    Text(&'a str),
    Parts(Vec<MessageContentPart<'a>>),
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum MessageContentPart<'a> {
    Text { text: &'a str },
    ImageUrl { image_url: ImageUrl },
}

#[derive(Serialize)]
struct ImageUrl {
    url: String,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    #[serde(default)]
    content: String,
}

fn user_content<'a>(prompt: &'a str, image_base64: Option<&'a str>) -> MessageContent<'a> {
    let Some(image_base64) = image_base64 else {
        return MessageContent::Text(prompt);
    };

    MessageContent::Parts(vec![
        MessageContentPart::Text { text: prompt },
        MessageContentPart::ImageUrl {
            image_url: ImageUrl {
                url: format!("data:image/jpeg;base64,{image_base64}"),
            },
        },
    ])
}
