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
    config: &'a Config,
}

impl<'a> OpenAiCompatClient<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl LlmClient for OpenAiCompatClient<'_> {
    async fn generate(&self, request: LlmRequest<'_>) -> anyhow::Result<LlmResponse> {
        let api_key = self.config.openai_compat_api_key.trim();
        if api_key.is_empty() {
            anyhow::bail!("OPENAI_COMPAT_API_KEY is empty");
        }

        let config = OpenAIConfig::new()
            .with_api_base(self.config.openai_compat_base_url.trim_end_matches('/'))
            .with_api_key(api_key);
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
