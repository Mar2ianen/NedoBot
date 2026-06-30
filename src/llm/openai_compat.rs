use async_openai::{
    Client,
    config::OpenAIConfig,
    types::chat::{
        ChatCompletionRequestMessageContentPartImage, ChatCompletionRequestMessageContentPartText,
        ChatCompletionRequestUserMessageArgs, ChatCompletionRequestUserMessageContent,
        ChatCompletionRequestUserMessageContentPart, CreateChatCompletionRequestArgs, ImageUrl,
    },
};
use async_trait::async_trait;

use crate::config::Config;
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

        let config = OpenAIConfig::new()
            .with_api_base(self.api_base)
            .with_api_key(self.api_key);
        let client = Client::with_config(config);
        let message = ChatCompletionRequestUserMessageArgs::default()
            .content(user_content(request.prompt, request.image_base64))
            .build()?;
        let response = client
            .chat()
            .create(
                CreateChatCompletionRequestArgs::default()
                    .model(request.model)
                    .messages([message.into()])
                    .temperature(request.temperature)
                    .max_completion_tokens(request.num_predict)
                    .build()?,
            )
            .await?;

        let content = response
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .unwrap_or_default();

        if content.trim().is_empty() {
            anyhow::bail!("empty OpenAI-compatible response");
        }

        Ok(LlmResponse { content })
    }
}

fn user_content(
    prompt: &str,
    image_base64: Option<&str>,
) -> ChatCompletionRequestUserMessageContent {
    let Some(image_base64) = image_base64 else {
        return prompt.into();
    };

    vec![
        ChatCompletionRequestMessageContentPartText {
            text: prompt.to_string(),
        }
        .into(),
        ChatCompletionRequestUserMessageContentPart::ImageUrl(
            ChatCompletionRequestMessageContentPartImage {
                image_url: ImageUrl {
                    url: format!("data:image/jpeg;base64,{image_base64}"),
                    detail: None,
                },
            },
        ),
    ]
    .into()
}
