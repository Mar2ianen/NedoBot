use std::time::Duration;

use async_openai::{
    Client,
    config::OpenAIConfig,
    error::OpenAIError,
    types::chat::{
        ChatCompletionRequestMessage, ChatCompletionRequestMessageContentPartImage,
        ChatCompletionRequestMessageContentPartText, ChatCompletionRequestSystemMessage,
        ChatCompletionRequestSystemMessageContent, ChatCompletionRequestUserMessage,
        ChatCompletionRequestUserMessageContent, ChatCompletionRequestUserMessageContentPart,
        CreateChatCompletionRequest, CreateChatCompletionRequestArgs, ImageUrl, ResponseFormat,
        ResponseFormatJsonSchema,
    },
};
use async_trait::async_trait;
use reqwest::header::USER_AGENT;

use crate::config::Config;
use crate::http;
use crate::llm::types::{LlmClient, LlmRequest, LlmResponse, LlmTransportError};

pub struct OpenAiCompatClient {
    client: Client<OpenAIConfig>,
}

impl OpenAiCompatClient {
    pub fn new(api_base: &str, api_key: &str, timeout: Duration) -> anyhow::Result<Self> {
        if api_key.trim().is_empty() {
            return Err(LlmTransportError::configuration().into());
        }

        let config = OpenAIConfig::new()
            .with_api_base(api_base.trim_end_matches('/'))
            .with_api_key(api_key.trim())
            .with_header(USER_AGENT, "tg-ai-bot-teloxide/0.1")
            .map_err(|err: OpenAIError| {
                anyhow::anyhow!("failed to set OpenAI-compatible User-Agent: {err}")
            })?;
        let http_client = http::client(timeout)?;
        Ok(Self {
            client: Client::with_config(config).with_http_client(http_client),
        })
    }

    pub fn from_config(config: &Config) -> anyhow::Result<Self> {
        Self::new(
            &config.openai_compat_base_url,
            &config.openai_compat_api_key,
            Duration::from_secs(45),
        )
    }
}

#[async_trait]
impl LlmClient for OpenAiCompatClient {
    async fn generate(&self, request: LlmRequest<'_>) -> anyhow::Result<LlmResponse> {
        let response = self
            .client
            .chat()
            .create(build_request(request)?)
            .await
            .map_err(map_openai_error)?;
        let content = response
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .filter(|content| !content.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("empty OpenAI-compatible response"))?;

        Ok(LlmResponse { content })
    }
}

fn map_openai_error(error: OpenAIError) -> anyhow::Error {
    match error {
        OpenAIError::ApiError(response) => {
            LlmTransportError::http_status(response.status_code.as_u16()).into()
        }
        OpenAIError::Reqwest(error) => match error.status() {
            Some(status) => LlmTransportError::http_status(status.as_u16()).into(),
            None => error.into(),
        },
        error => error.into(),
    }
}

fn build_request(request: LlmRequest<'_>) -> anyhow::Result<CreateChatCompletionRequest> {
    let mut messages = Vec::new();
    if let Some(system_prompt) = request.system_prompt {
        messages.push(ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessage {
                content: ChatCompletionRequestSystemMessageContent::Text(system_prompt.to_string()),
                name: None,
            },
        ));
    }
    messages.push(ChatCompletionRequestMessage::User(
        ChatCompletionRequestUserMessage {
            content: user_content(request.prompt, request.image_base64),
            name: None,
        },
    ));

    let mut builder = CreateChatCompletionRequestArgs::default();
    builder
        .model(request.model)
        .messages(messages)
        .temperature(request.temperature)
        .max_completion_tokens(request.num_predict);
    if let Some(output) = request.structured_output {
        builder.response_format(response_format(output));
    }
    builder.build().map_err(Into::into)
}

fn response_format(output: crate::llm::types::StructuredOutput<'_>) -> ResponseFormat {
    ResponseFormat::JsonSchema {
        json_schema: ResponseFormatJsonSchema {
            description: None,
            name: output.name.to_string(),
            schema: output.schema.clone(),
            strict: Some(true),
        },
    }
}

fn user_content(
    prompt: &str,
    image_base64: Option<&str>,
) -> ChatCompletionRequestUserMessageContent {
    let Some(image_base64) = image_base64 else {
        return ChatCompletionRequestUserMessageContent::Text(prompt.to_string());
    };

    ChatCompletionRequestUserMessageContent::Array(vec![
        ChatCompletionRequestUserMessageContentPart::Text(
            ChatCompletionRequestMessageContentPartText {
                text: prompt.to_string(),
            },
        ),
        ChatCompletionRequestUserMessageContentPart::ImageUrl(
            ChatCompletionRequestMessageContentPartImage {
                image_url: ImageUrl {
                    url: format!("data:image/jpeg;base64,{image_base64}"),
                    detail: None,
                },
            },
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, header},
        routing::post,
    };
    use serde_json::{Value, json};
    use tokio::sync::mpsc;

    struct CapturedRequest {
        headers: HeaderMap,
        body: Value,
    }

    async fn capture_request(
        State(sender): State<mpsc::UnboundedSender<CapturedRequest>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        sender.send(CapturedRequest { headers, body }).unwrap();
        Json(json!({
            "id": "test-completion",
            "object": "chat.completion",
            "created": 0,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "OK"},
                "finish_reason": "stop"
            }],
            "usage": null
        }))
    }

    fn llm_request<'a>(
        structured_output: Option<crate::llm::types::StructuredOutput<'a>>,
    ) -> LlmRequest<'a> {
        LlmRequest {
            model: "gemma-4",
            system_prompt: Some("system"),
            prompt: "prompt",
            image_base64: Some("base64-image"),
            temperature: 0.0,
            num_predict: 256,
            structured_output,
        }
    }

    #[test]
    fn text_request_omits_response_format() {
        let body = serde_json::to_value(build_request(llm_request(None)).unwrap()).unwrap();

        assert!(body.get("response_format").is_none());
        assert_eq!(body["max_completion_tokens"], 256);
        assert_eq!(
            body["messages"][0],
            json!({"role": "system", "content": "system"})
        );
        assert_eq!(body["messages"][1]["content"][1]["type"], "image_url");
    }

    #[tokio::test]
    async fn client_emits_authorization_user_agent_image_and_schema_over_http() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let app = Router::new()
            .route("/v1/chat/completions", post(capture_request))
            .with_state(sender);
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let schema = json!({"type": "object", "additionalProperties": false});
        let request = llm_request(Some(crate::llm::types::StructuredOutput {
            name: "wire_schema",
            schema: &schema,
        }));
        let client = OpenAiCompatClient::new(
            &format!("http://{address}/v1"),
            "test-compatible-key",
            Duration::from_secs(5),
        )
        .unwrap();

        assert_eq!(client.generate(request).await.unwrap().content, "OK");
        let captured = receiver.recv().await.unwrap();
        server.abort();

        assert_eq!(
            captured.headers.get(header::AUTHORIZATION).unwrap(),
            "Bearer test-compatible-key"
        );
        assert_eq!(
            captured.headers.get(header::USER_AGENT).unwrap(),
            "tg-ai-bot-teloxide/0.1"
        );
        assert_eq!(captured.body["max_completion_tokens"], 256);
        assert_eq!(
            captured.body["messages"][1]["content"][1]["type"],
            "image_url"
        );
        assert_eq!(
            captured.body["response_format"]["json_schema"]["name"],
            "wire_schema"
        );
    }

    #[test]
    fn structured_request_uses_strict_json_schema() {
        let schema = json!({"type": "object", "additionalProperties": false});
        let request = llm_request(Some(crate::llm::types::StructuredOutput {
            name: "avatar_profile_assessment",
            schema: &schema,
        }));
        let body = serde_json::to_value(build_request(request).unwrap()).unwrap();

        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["name"],
            "avatar_profile_assessment"
        );
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert_eq!(body["response_format"]["json_schema"]["schema"], schema);
    }
}
