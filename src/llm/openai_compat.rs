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

        let mut messages = Vec::new();
        if let Some(system_prompt) = request.system_prompt {
            messages.push(ChatMessage {
                role: "system",
                content: MessageContent::Text(system_prompt),
            });
        }
        messages.push(ChatMessage {
            role: "user",
            content: user_content(request.prompt, request.image_base64),
        });

        let body = ChatCompletionRequest {
            model: request.model,
            messages,
            temperature: request.temperature,
            max_completion_tokens: request.num_predict,
            response_format: request
                .structured_output
                .map(|output| ResponseFormat::json_schema(output.name, output.schema)),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat<'a>>,
}

#[derive(Serialize)]
struct ResponseFormat<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    json_schema: JsonSchemaResponseFormat<'a>,
}

#[derive(Serialize)]
struct JsonSchemaResponseFormat<'a> {
    name: &'a str,
    strict: bool,
    schema: &'a serde_json::Value,
}

impl<'a> ResponseFormat<'a> {
    fn json_schema(name: &'a str, schema: &'a serde_json::Value) -> Self {
        Self {
            kind: "json_schema",
            json_schema: JsonSchemaResponseFormat {
                name,
                strict: true,
                schema,
            },
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(response_format: Option<ResponseFormat<'_>>) -> ChatCompletionRequest<'_> {
        ChatCompletionRequest {
            model: "gemma-4",
            messages: Vec::new(),
            temperature: 0.0,
            max_completion_tokens: 256,
            response_format,
        }
    }

    #[test]
    fn text_request_omits_response_format() {
        let body = serde_json::to_value(request(None)).unwrap();
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn structured_request_uses_strict_json_schema() {
        let schema = json!({"type": "object", "additionalProperties": false});
        let body = serde_json::to_value(request(Some(ResponseFormat::json_schema(
            "avatar_profile_assessment",
            &schema,
        ))))
        .unwrap();

        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["name"],
            "avatar_profile_assessment"
        );
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert_eq!(body["response_format"]["json_schema"]["schema"], schema);
    }
}
