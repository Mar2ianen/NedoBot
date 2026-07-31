use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use genai::adapter::AdapterKind;
use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatResponse, ChatResponseFormat, ContentPart, JsonSpec,
    MessageContent, ReasoningEffort, StopReason, Tool,
};
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ModelIden, ServiceTarget};

use crate::http;
use crate::llm::profiles::{Egress, GenAiAdapter, StructuredOutputMode, ThinkingMode};
use crate::llm::types::{LlmTransportError, StructuredOutput};

const GENAI_HTTP_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct GenAiTransport {
    direct: Client,
    proxied: Option<Client>,
}

#[derive(Clone, Copy)]
pub struct ModelTarget<'a> {
    pub adapter: GenAiAdapter,
    pub endpoint: &'a str,
    pub api_key: &'a str,
    pub model: &'a str,
}

#[derive(Clone, Copy)]
pub struct ImageInput<'a> {
    pub mime_type: &'a str,
    pub base64: &'a str,
    pub file_name: Option<&'a str>,
}

pub struct GenAiRequest<'a> {
    pub model: ModelTarget<'a>,
    pub system_prompt: Option<&'a str>,
    pub prompt: &'a str,
    pub image: Option<ImageInput<'a>>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub timeout: Duration,
    pub reasoning: ThinkingMode,
    pub reasoning_budget: Option<u32>,
    pub structured_output_mode: StructuredOutputMode,
    pub structured_output: Option<StructuredOutput<'a>>,
    pub extra_body: Option<serde_json::Value>,
    pub egress: Egress,
}

#[allow(dead_code)] // Используется фазой B для native tool-call history.
pub struct GenAiChatRequest<'a> {
    pub model: ModelTarget<'a>,
    pub system_prompt: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub tools: Option<Vec<Tool>>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub timeout: Duration,
    pub reasoning: ThinkingMode,
    pub reasoning_budget: Option<u32>,
    pub egress: Egress,
}

static TRANSPORTS: LazyLock<Mutex<HashMap<Option<String>, Arc<GenAiTransport>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl GenAiTransport {
    pub fn new(direct_http: reqwest::Client, proxied_http: Option<reqwest::Client>) -> Self {
        Self {
            direct: Client::builder().with_reqwest(direct_http).build(),
            proxied: proxied_http.map(|http| Client::builder().with_reqwest(http).build()),
        }
    }

    /// Получает долгоживущий transport из process-local cache. Ключом служит только proxy URL;
    /// API keys в cache и его отладочном представлении не хранятся отдельно.
    pub fn cached(proxy_url: Option<&str>) -> anyhow::Result<Arc<Self>> {
        let proxy_url = proxy_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let mut transports = TRANSPORTS
            .lock()
            .map_err(|_| anyhow::anyhow!("LLM transport cache lock is poisoned"))?;
        if let Some(transport) = transports.get(&proxy_url) {
            return Ok(Arc::clone(transport));
        }

        let direct_http = http::client(GENAI_HTTP_TIMEOUT)?;
        let proxied_http = proxy_url
            .as_deref()
            .map(|proxy| http::client_with_proxy(GENAI_HTTP_TIMEOUT, Some(proxy)))
            .transpose()?;
        let transport = Arc::new(Self::new(direct_http, proxied_http));
        transports.insert(proxy_url, Arc::clone(&transport));
        Ok(transport)
    }

    pub async fn generate(&self, request: GenAiRequest<'_>) -> Result<String, LlmTransportError> {
        let structured = request.structured_output.is_some();
        let chat_request =
            build_chat_request(request.system_prompt, request.prompt, request.image, None);
        let options = build_chat_options(
            request.temperature,
            request.max_tokens,
            request.reasoning,
            request.reasoning_budget,
            request.structured_output_mode,
            request.structured_output,
            request.extra_body,
        );
        let response = self
            .exec_chat(
                request.model,
                chat_request,
                options,
                request.timeout,
                request.egress,
                structured,
            )
            .await?;
        if response
            .stop_reason
            .as_ref()
            .is_some_and(StopReason::is_max_tokens)
        {
            return Err(LlmTransportError::invalid_response());
        }
        if !response.tool_calls().is_empty() {
            return Err(LlmTransportError::invalid_response());
        }
        response
            .first_text()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
            .ok_or_else(LlmTransportError::empty_response)
    }

    #[allow(dead_code)] // Используется фазой B для native tool-call history.
    pub async fn chat(
        &self,
        request: GenAiChatRequest<'_>,
    ) -> Result<ChatResponse, LlmTransportError> {
        let chat_request = ChatRequest {
            system: request.system_prompt,
            messages: request.messages,
            tools: request.tools,
            previous_response_id: None,
            store: None,
        };
        let options = build_chat_options(
            request.temperature,
            request.max_tokens,
            request.reasoning,
            request.reasoning_budget,
            StructuredOutputMode::PromptOnly,
            None,
            None,
        );
        self.exec_chat(
            request.model,
            chat_request,
            options,
            request.timeout,
            request.egress,
            false,
        )
        .await
    }

    async fn exec_chat(
        &self,
        model: ModelTarget<'_>,
        request: ChatRequest,
        options: ChatOptions,
        timeout_duration: Duration,
        egress: Egress,
        structured_output: bool,
    ) -> Result<ChatResponse, LlmTransportError> {
        let endpoint = format!("{}/", model.endpoint.trim_end_matches('/'));
        let target = ServiceTarget {
            endpoint: Endpoint::from_owned(endpoint),
            auth: AuthData::from_single(model.api_key),
            model: ModelIden::new(adapter_kind(model.adapter), model.model),
        };
        let client = match egress {
            Egress::Direct => &self.direct,
            Egress::Proxy => self
                .proxied
                .as_ref()
                .ok_or_else(LlmTransportError::configuration)?,
        };
        let result = tokio::time::timeout(
            timeout_duration,
            client.exec_chat(target, request, Some(&options)),
        )
        .await
        .map_err(|_| LlmTransportError::timeout())?;
        result.map_err(|error| map_genai_error(error, structured_output))
    }
}

fn build_chat_request(
    system_prompt: Option<&str>,
    prompt: &str,
    image: Option<ImageInput<'_>>,
    tools: Option<Vec<Tool>>,
) -> ChatRequest {
    let user_content = match image {
        Some(image) => MessageContent::from_parts(vec![
            ContentPart::from_text(prompt),
            ContentPart::from_binary_base64(
                image.mime_type,
                Arc::<str>::from(image.base64),
                image.file_name.map(str::to_owned),
            ),
        ]),
        None => MessageContent::from(prompt),
    };
    ChatRequest {
        system: system_prompt.map(str::to_owned),
        messages: vec![ChatMessage::user(user_content)],
        tools,
        previous_response_id: None,
        store: None,
    }
}

fn build_chat_options(
    temperature: f32,
    max_tokens: u32,
    reasoning: ThinkingMode,
    reasoning_budget: Option<u32>,
    structured_output_mode: StructuredOutputMode,
    structured_output: Option<StructuredOutput<'_>>,
    extra_body: Option<serde_json::Value>,
) -> ChatOptions {
    let mut options = ChatOptions::default().with_max_tokens(max_tokens);
    if reasoning != ThinkingMode::LevelLow {
        options = options.with_temperature(f64::from(temperature));
    }
    options = match reasoning {
        ThinkingMode::None => options,
        ThinkingMode::Budget => options.with_reasoning_effort(ReasoningEffort::Budget(
            reasoning_budget.unwrap_or_default(),
        )),
        ThinkingMode::LevelLow => options.with_reasoning_effort(ReasoningEffort::Low),
    };
    if let Some(structured_output) = structured_output {
        options = match structured_output_mode {
            StructuredOutputMode::JsonSchema => {
                options.with_response_format(ChatResponseFormat::JsonSpec(JsonSpec::new(
                    structured_output.name,
                    structured_output.schema.clone(),
                )))
            }
            StructuredOutputMode::JsonObject => {
                options.with_response_format(ChatResponseFormat::JsonMode)
            }
            StructuredOutputMode::PromptOnly => options,
        };
    }
    if let Some(extra_body) = extra_body {
        options = options.with_extra_body(extra_body);
    }
    options
}

fn adapter_kind(adapter: GenAiAdapter) -> AdapterKind {
    match adapter {
        GenAiAdapter::OpenAi => AdapterKind::OpenAI,
        GenAiAdapter::Gemini => AdapterKind::Gemini,
        GenAiAdapter::Groq => AdapterKind::Groq,
        GenAiAdapter::OpenRouter => AdapterKind::OpenRouter,
        GenAiAdapter::OllamaCloud => AdapterKind::OllamaCloud,
    }
}

fn map_genai_error(error: genai::Error, structured_output: bool) -> LlmTransportError {
    match &error {
        genai::Error::WebModelCall { webc_error, .. }
        | genai::Error::WebAdapterCall { webc_error, .. } => {
            map_web_error(webc_error, structured_output)
        }
        genai::Error::NoChatResponse { .. } => LlmTransportError::empty_response(),
        genai::Error::RequiresApiKey { .. }
        | genai::Error::NoAuthResolver { .. }
        | genai::Error::NoAuthData { .. }
        | genai::Error::Resolver { .. } => LlmTransportError::configuration(),
        genai::Error::AdapterNotSupported { .. }
        | genai::Error::MessageContentTypeNotSupported { .. } => {
            LlmTransportError::unsupported_feature()
        }
        _ => LlmTransportError::invalid_response(),
    }
}

fn map_web_error(error: &genai::webc::Error, structured_output: bool) -> LlmTransportError {
    match error {
        genai::webc::Error::ResponseFailedStatus { status, .. } => {
            if structured_output && matches!(status.as_u16(), 400 | 422) {
                LlmTransportError::structured_output_rejected()
            } else {
                LlmTransportError::http_status(status.as_u16())
            }
        }
        genai::webc::Error::Reqwest(error) if error.is_timeout() => LlmTransportError::timeout(),
        genai::webc::Error::Reqwest(_) => LlmTransportError::invalid_response(),
        genai::webc::Error::ResponseFailedInvalidJson { .. }
        | genai::webc::Error::ResponseFailedNotJson { .. }
        | genai::webc::Error::JsonValueExt(_) => LlmTransportError::invalid_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_request_preserves_mime_and_filename() {
        let request = build_chat_request(
            Some("system"),
            "prompt",
            Some(ImageInput {
                mime_type: "image/png",
                base64: "encoded",
                file_name: Some("avatar.png"),
            }),
            None,
        );
        assert_eq!(request.system.as_deref(), Some("system"));
        let binary = request.messages[0]
            .content
            .clone()
            .into_binaries()
            .remove(0);
        assert_eq!(binary.content_type, "image/png");
        assert_eq!(binary.name.as_deref(), Some("avatar.png"));
    }

    #[test]
    fn structured_output_modes_map_to_genai_response_formats() {
        let schema = serde_json::json!({"type": "object"});
        let output = StructuredOutput {
            name: "result",
            schema: &schema,
        };
        let json_schema = build_chat_options(
            0.2,
            100,
            ThinkingMode::None,
            None,
            StructuredOutputMode::JsonSchema,
            Some(output),
            None,
        );
        assert!(matches!(
            json_schema.response_format,
            Some(ChatResponseFormat::JsonSpec(_))
        ));
        let json_object = build_chat_options(
            0.2,
            100,
            ThinkingMode::None,
            None,
            StructuredOutputMode::JsonObject,
            Some(output),
            None,
        );
        assert!(matches!(
            json_object.response_format,
            Some(ChatResponseFormat::JsonMode)
        ));
        let prompt_only = build_chat_options(
            0.2,
            100,
            ThinkingMode::None,
            None,
            StructuredOutputMode::PromptOnly,
            Some(output),
            None,
        );
        assert!(prompt_only.response_format.is_none());
    }

    #[test]
    fn reasoning_mapping_does_not_add_temperature_for_low_level() {
        let options = build_chat_options(
            0.2,
            100,
            ThinkingMode::LevelLow,
            None,
            StructuredOutputMode::PromptOnly,
            None,
            None,
        );
        assert_eq!(options.temperature, None);
        assert!(matches!(
            options.reasoning_effort,
            Some(ReasoningEffort::Low)
        ));
    }

    #[test]
    fn safe_mapping_never_requires_formatting_provider_error() {
        let error = genai::Error::NoChatResponse {
            model_iden: ModelIden::new(AdapterKind::OpenAI, "secret-model"),
        };
        assert_eq!(
            map_genai_error(error, false),
            LlmTransportError::EmptyResponse
        );
    }
}
