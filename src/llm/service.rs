use genai::chat::{ChatMessage, ChatResponse, Tool};

use crate::config::Config;
use crate::llm::genai_transport::{
    GenAiChatRequest, GenAiRequest, GenAiTransport, ImageInput, ModelTarget,
};
use crate::llm::profiles::{RouteRequirements, RouteSelection};
use crate::llm::types::{GeneratedText, LlmAttempt, LlmTransportError, StructuredOutput};

pub type OutputValidator = dyn Fn(&str) -> anyhow::Result<()> + Send + Sync;

pub struct GenerateTextOptions<'a> {
    /// Имя task route из authoritative `LLM_PROFILES_PATH`.
    pub route: &'a str,
    pub system_prompt: Option<&'a str>,
    pub prompt: &'a str,
    pub image_base64: Option<&'a str>,
    pub temperature: f32,
    pub num_predict: u32,
    pub output_validator: Option<&'a OutputValidator>,
    pub structured_output: Option<StructuredOutput<'a>>,
}

pub struct GenerateChatOptions<'a> {
    /// Имя task route из authoritative `LLM_PROFILES_PATH`.
    pub route: &'a str,
    pub system_prompt: Option<&'a str>,
    pub messages: Vec<ChatMessage>,
    pub tools: Option<Vec<Tool>>,
    pub requires_tools: bool,
    pub previous_response_id: Option<String>,
    pub temperature: f32,
    pub num_predict: u32,
}

const VALIDATION_RETRY_ATTEMPTS: usize = 1;

struct GenerateOnceRequest<'a> {
    system_prompt: Option<&'a str>,
    prompt: &'a str,
    image_base64: Option<&'a str>,
    temperature: f32,
    num_predict: u32,
    structured_output: Option<StructuredOutput<'a>>,
}

pub async fn generate_chat_checked(
    config: &Config,
    options: GenerateChatOptions<'_>,
) -> anyhow::Result<ChatResponse> {
    let profiles = config
        .llm_profiles
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("LLM_PROFILES_PATH must configure authoritative routes"))?;
    generate_chat_with_profile_checked(config, profiles, options).await
}

pub async fn generate_text_checked(
    config: &Config,
    options: GenerateTextOptions<'_>,
) -> anyhow::Result<GeneratedText> {
    let profiles = config
        .llm_profiles
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("LLM_PROFILES_PATH must configure authoritative routes"))?;
    generate_text_with_profile_checked(config, profiles, options).await
}

async fn generate_text_with_profile_checked(
    config: &Config,
    profiles: &crate::llm::profiles::LlmProfiles,
    options: GenerateTextOptions<'_>,
) -> anyhow::Result<GeneratedText> {
    let route = options.route;
    let requirements = RouteRequirements {
        requires_images: options.image_base64.is_some(),
        requires_system_prompt: options.system_prompt.is_some(),
        num_predict: Some(options.num_predict),
        // Любой из режимов профиля задаёт transport contract. PromptOnly намеренно
        // допустим: JSON-контракт остаётся в prompt и проверяется validator-ом.
        structured_output: None,
        ..RouteRequirements::default()
    };
    let resolved = profiles.resolve_route(route, &requirements)?;
    let mut last_error = None;
    let mut attempts = Vec::new();

    for (fallback_index, selection) in resolved.selections.iter().enumerate() {
        let mut attempt_prompt = options.prompt.to_string();
        for attempt in 0..=VALIDATION_RETRY_ATTEMPTS {
            let generation = generate_profile_once(
                config,
                selection,
                GenerateOnceRequest {
                    system_prompt: options.system_prompt,
                    prompt: &attempt_prompt,
                    image_base64: options.image_base64,
                    temperature: options.temperature,
                    num_predict: options.num_predict,
                    structured_output: options.structured_output,
                },
            )
            .await;
            match generation {
                Ok(mut generation) => {
                    let llm_attempt = generation.attempts.pop().expect("generation has attempt");
                    if let Some(validate) = options.output_validator
                        && let Err(err) = validate(&generation.content)
                    {
                        attempts.push(LlmAttempt {
                            outcome: "validation_failed".to_string(),
                            ..llm_attempt
                        });
                        last_error = Some(err);
                        if attempt < VALIDATION_RETRY_ATTEMPTS {
                            attempt_prompt = validation_retry_prompt(
                                options.prompt,
                                &format!("{:#}", last_error.as_ref().expect("validation error")),
                            );
                            continue;
                        }
                        if !resolved.fallback_on_validation_failure {
                            return Err(last_error.expect("validation error"));
                        }
                        break;
                    }
                    attempts.push(llm_attempt);
                    generation.attempts = attempts;
                    if fallback_index > 0 {
                        tracing::info!(
                            route,
                            fallback_index,
                            provider = selection.provider_key,
                            model = selection.model.model,
                            "LLM profile fallback succeeded"
                        );
                    }
                    return Ok(generation);
                }
                Err(err) => {
                    let empty_response = is_empty_response(&err);
                    attempts.push(LlmAttempt {
                        provider: selection.provider_key.to_string(),
                        model: selection.model.model.clone(),
                        outcome: classify_attempt_error(&err),
                    });
                    last_error = Some(err);
                    if empty_response && attempt < VALIDATION_RETRY_ATTEMPTS {
                        attempt_prompt = validation_retry_prompt(
                            options.prompt,
                            "модель вернула пустой ответ; верни полный ответ по исходному контракту",
                        );
                        continue;
                    }
                    break;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        anyhow::anyhow!("no compatible LLM profile generation attempts were configured")
    }))
}

async fn generate_chat_with_profile_checked(
    config: &Config,
    profiles: &crate::llm::profiles::LlmProfiles,
    options: GenerateChatOptions<'_>,
) -> anyhow::Result<ChatResponse> {
    let route = options.route;
    let requirements = RouteRequirements {
        requires_tools: options.requires_tools || options.tools.is_some(),
        requires_system_prompt: options.system_prompt.is_some(),
        num_predict: Some(options.num_predict),
        ..RouteRequirements::default()
    };
    let resolved = profiles.resolve_route(route, &requirements)?;
    let mut last_error = None;

    for (fallback_index, selection) in resolved.selections.iter().enumerate() {
        match generate_chat_profile_once(config, selection, &options).await {
            Ok(response) => {
                if fallback_index > 0 {
                    tracing::info!(
                        route,
                        fallback_index,
                        provider = selection.provider_key,
                        model = selection.model.model,
                        "LLM chat profile fallback succeeded"
                    );
                }
                return Ok(response);
            }
            Err(error) => {
                tracing::warn!(
                    route,
                    fallback_index,
                    provider = selection.provider_key,
                    model = selection.model.model,
                    outcome = classify_attempt_error(&error),
                    "LLM chat profile attempt failed"
                );
                last_error = Some(error);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        anyhow::anyhow!("no compatible LLM profile chat attempts were configured")
    }))
}

async fn generate_chat_profile_once(
    config: &Config,
    selection: &RouteSelection<'_>,
    options: &GenerateChatOptions<'_>,
) -> anyhow::Result<ChatResponse> {
    let api_key = std::env::var(&selection.provider.api_key_env).map_err(|_| {
        anyhow::anyhow!(
            "LLM profile provider {:?} requires configured secret {}",
            selection.provider_key,
            selection.provider.api_key_env
        )
    })?;
    if api_key.trim().is_empty() {
        anyhow::bail!(
            "LLM profile provider {:?} requires configured secret {}",
            selection.provider_key,
            selection.provider.api_key_env
        );
    }
    let transport = GenAiTransport::cached(config.llm_proxy_url.as_deref())?;
    transport
        .chat(GenAiChatRequest {
            model: ModelTarget {
                adapter: selection.provider.genai_adapter(),
                endpoint: &selection.provider.base_url,
                api_key: &api_key,
                model: &selection.model.model,
            },
            system_prompt: options.system_prompt.map(str::to_owned),
            messages: options.messages.clone(),
            tools: options.tools.clone(),
            previous_response_id: options.previous_response_id.clone(),
            temperature: options.temperature,
            max_tokens: options.num_predict,
            timeout: std::time::Duration::from_secs(selection.capabilities.request_timeout_sec),
            reasoning: selection.capabilities.thinking,
            reasoning_budget: Some(config.gemini_thinking_budget),
            egress: selection.provider.egress,
        })
        .await
        .map_err(anyhow::Error::new)
}

async fn generate_profile_once(
    config: &Config,
    selection: &RouteSelection<'_>,
    request: GenerateOnceRequest<'_>,
) -> anyhow::Result<GeneratedText> {
    let GenerateOnceRequest {
        system_prompt,
        prompt,
        image_base64,
        temperature,
        num_predict,
        structured_output,
        ..
    } = request;
    let api_key = std::env::var(&selection.provider.api_key_env).map_err(|_| {
        anyhow::anyhow!(
            "LLM profile provider {:?} requires configured secret {}",
            selection.provider_key,
            selection.provider.api_key_env
        )
    })?;
    if api_key.trim().is_empty() {
        anyhow::bail!(
            "LLM profile provider {:?} requires configured secret {}",
            selection.provider_key,
            selection.provider.api_key_env
        );
    }
    let image_base64 = image_base64.filter(|_| selection.capabilities.supports_images);
    let timeout = std::time::Duration::from_secs(selection.capabilities.request_timeout_sec);
    let image = image_base64.map(|base64| ImageInput {
        mime_type: "image/jpeg",
        base64,
        file_name: Some("image.jpg"),
    });
    let transport = GenAiTransport::cached(config.llm_proxy_url.as_deref())?;
    let response = transport
        .generate(GenAiRequest {
            model: ModelTarget {
                adapter: selection.provider.genai_adapter(),
                endpoint: &selection.provider.base_url,
                api_key: &api_key,
                model: &selection.model.model,
            },
            system_prompt,
            prompt,
            image,
            temperature,
            max_tokens: num_predict,
            timeout,
            reasoning: selection.capabilities.thinking,
            reasoning_budget: Some(config.gemini_thinking_budget),
            structured_output_mode: selection.capabilities.structured_output,
            structured_output,
            extra_body: None,
            egress: selection.provider.egress,
        })
        .await
        .map_err(anyhow::Error::new)?;
    Ok(GeneratedText {
        provider: selection.provider_key.to_string(),
        model: selection.model.model.clone(),
        content: response,
        image_used: image_base64.is_some(),
        attempts: vec![LlmAttempt {
            provider: selection.provider_key.to_string(),
            model: selection.model.model.clone(),
            outcome: "success".to_string(),
        }],
    })
}

fn classify_attempt_error(error: &anyhow::Error) -> String {
    match error.downcast_ref::<LlmTransportError>() {
        Some(LlmTransportError::HttpStatus(429)) => "http_429".to_string(),
        Some(LlmTransportError::HttpStatus(status)) if *status >= 500 => "http_5xx".to_string(),
        Some(LlmTransportError::HttpStatus(status)) => format!("http_{status}"),
        Some(LlmTransportError::Configuration) => "configuration".to_string(),
        Some(LlmTransportError::Timeout) => "timeout".to_string(),
        Some(LlmTransportError::EmptyResponse) => "empty_response".to_string(),
        Some(LlmTransportError::InvalidResponse) => "invalid_response".to_string(),
        Some(LlmTransportError::UnsupportedFeature) => "unsupported_feature".to_string(),
        Some(LlmTransportError::StructuredOutputRejected) => {
            "structured_output_fallback".to_string()
        }
        None if error
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_timeout) =>
        {
            "timeout".to_string()
        }
        None => "error".to_string(),
    }
}

fn is_empty_response(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<LlmTransportError>(),
        Some(LlmTransportError::EmptyResponse)
    )
}

fn validation_retry_prompt(original_prompt: &str, validation_error: &str) -> String {
    format!(
        "{original_prompt}\n\nПредыдущий ответ не прошёл автоматическую проверку: {validation_error}. Верни новый ответ, строго соблюдая формат, ограничения длины и обязательные токены из системных правил."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SearchMcpTools;
    use crate::llm::profiles::LlmProfiles;
    use axum::{
        Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post,
    };
    use serde_json::{Value, json};
    use std::sync::LazyLock;
    use tokio::sync::{Mutex, mpsc};

    static PROFILE_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn config() -> Config {
        Config {
            source_channel_id: -1001,
            discussion_chat_id: -1002,
            chat_invite_url: "https://t.me/example".to_string(),
            chat_invite_label: "чат".to_string(),
            post_signature_marker: "marker".to_string(),
            llm_profiles_path: None,
            llm_profiles: None,
            llm_temperature: 0.35,
            llm_max_tokens: 90,
            llm_proxy_url: None,
            memory_llm_temperature: 0.2,
            memory_llm_max_tokens: 220,
            rag_enabled: false,
            rag_embedding_url: "http://127.0.0.1:8788".to_string(),
            rag_embedding_model: "cointegrated/rubert-tiny2".to_string(),
            rag_embedding_timeout_sec: 10,
            rag_top_k: 6,
            rag_min_similarity: 0.55,
            rag_temporal_half_life_days: 180.0,
            chat_retrieval_embeddings_enabled: false,
            chat_retrieval_embedding_batch_size: 16,
            chat_retrieval_embedding_poll_sec: 5,
            chat_retrieval_shadow_enabled: false,
            chat_retrieval_evidence_enabled: false,
            chat_retrieval_evidence_min_score: 2.0,
            chat_retrieval_window_days: 30,
            chat_retrieval_half_life_days: 7.0,
            search_enabled: false,
            search_extract_temperature: 0.1,
            search_extract_max_tokens: 700,
            search_mcp_command: None,
            search_mcp_args: Vec::new(),
            search_mcp_env: Vec::new(),
            search_mcp_timeout_sec: 8,
            search_query_timeout_sec: 8,
            search_mcp_tools: SearchMcpTools {
                web: "web_search".to_string(),
                github: "github_search".to_string(),
                reddit: "reddit_search".to_string(),
            },
            search_mcp_fetch_tool: Some("web_fetch_exa".to_string()),
            search_fetch_top_n: 2,
            search_fetch_max_chars: 6000,
            comment_blocked_source_domains: vec!["meduza.io".to_string()],
            comment_blocked_terms: Vec::new(),
            search_github_mcp_command: None,
            search_github_mcp_args: Vec::new(),
            search_github_mcp_env: vec![
                "PATH".to_string(),
                "HOME".to_string(),
                "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
            ],
            search_github_mcp_tools: vec!["search_issues".to_string(), "search_code".to_string()],
            groq_api_key: String::new(),
            new_user_audit_enabled: false,
            new_user_audit_max_tokens: 900,
            gemini_thinking_budget: 1024,
            owner_telegram_id: None,
            send_owner_preview: false,
            ask_enabled: false,
            ask_allow_chat_admins: true,
            ask_private_user_ids: Vec::new(),
            ask_llm_temperature: 0.2,
            ask_llm_max_tokens: 1800,
            ask_max_steps: 5,
            ask_action_timeout_sec: 45,
            ask_total_timeout_sec: 180,
            ask_max_concurrency: 1,
            ask_db_mcp_command: None,
            ask_db_mcp_args: Vec::new(),
            ask_db_mcp_env: vec!["ASK_DATABASE_URL".to_string()],
            ask_db_mcp_timeout_sec: 8,
            profile_refresh_concurrency: 4,
            comment_custom_emoji_id: None,
            first_comment_max_image_mb: 10,
            tech_custom_emoji_id: None,
            amd_custom_emoji_id: None,
            radeon_custom_emoji_id: None,
            ryzen_custom_emoji_id: None,
            voice_transcription_enabled: false,
            voice_auto_transcribe: false,
            voice_max_duration_sec: 600,
            voice_max_file_mb: 20,
            voice_short_text_max_chars: 400,
            voice_language: "ru".to_string(),
            voice_asr_provider: "groq".to_string(),
            voice_asr_model: "whisper-large-v3-turbo".to_string(),
            voice_asr_temperature: 0.0,
            voice_cleanup_temperature: 0.2,
            voice_cleanup_max_tokens: 1800,
            voice_render_expandable_chapters: true,
            voice_send_full_file: true,
            public_base_url: None,
            static_files_dir: "/tmp/tg-ai-bot-static".to_string(),
        }
    }

    #[tokio::test]
    async fn profile_prompt_only_omits_response_format_and_validator_accepts_json_contract() {
        async fn capture(
            State(sender): State<mpsc::UnboundedSender<Value>>,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            sender.send(body).unwrap();
            Json(json!({
                "id": "profile-test",
                "object": "chat.completion",
                "created": 0,
                "model": "profile-model",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "{\"ok\":true}"},
                    "finish_reason": "stop"
                }]
            }))
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/v1/chat/completions", post(capture))
                    .with_state(sender),
            )
            .await
            .unwrap();
        });
        let mut config = config();
        config.llm_profiles = Some(
            LlmProfiles::from_toml(&format!(
                r#"
[providers.test]
driver = "openai_compatible"
base_url = "http://{address}/v1"
api_key_env = "PROFILE_PROMPT_ONLY_TEST_KEY"

[models.test]
provider = "test"
model = "profile-model"
[models.test.capabilities]
supports_images = false
supports_tools = false
supports_system_prompt = true
structured_output = "prompt_only"
context_window_tokens = 4096
max_output_tokens = 256
request_timeout_sec = 5
thinking = "none"

[routes.profile_test]
models = ["test"]
"#
            ))
            .unwrap(),
        );
        unsafe { std::env::set_var("PROFILE_PROMPT_ONLY_TEST_KEY", "test-key") };
        let schema = json!({"type": "object"});
        let validator = |content: &str| -> anyhow::Result<()> {
            let value: Value = serde_json::from_str(content)?;
            anyhow::ensure!(value["ok"] == true, "JSON contract is invalid");
            Ok(())
        };

        let generated = generate_text_checked(
            &config,
            GenerateTextOptions {
                route: "profile_test",
                system_prompt: Some("return a JSON object"),
                prompt: "{\"contract\":\"JSON only\"}",
                image_base64: None,
                temperature: 0.0,
                num_predict: 64,
                output_validator: Some(&validator),
                structured_output: Some(StructuredOutput {
                    name: "test_contract",
                    schema: &schema,
                }),
            },
        )
        .await
        .unwrap();

        assert_eq!(generated.provider, "test");
        assert_eq!(generated.model, "profile-model");
        assert_eq!(generated.content, "{\"ok\":true}");
        let request = receiver.recv().await.unwrap();
        assert!(request.get("response_format").is_none());
        assert_eq!(request["model"], "profile-model");
        unsafe { std::env::remove_var("PROFILE_PROMPT_ONLY_TEST_KEY") };
        server.abort();
    }

    fn switching_profiles(
        address: std::net::SocketAddr,
        primary_path: &str,
        fallback_path: &str,
        fallback_on_validation_failure: bool,
    ) -> LlmProfiles {
        LlmProfiles::from_toml(&format!(
            r#"
[providers.primary]
driver = "openai_compatible"
base_url = "http://{address}/{primary_path}/v1"
api_key_env = "PROFILE_SWITCH_PRIMARY_KEY"
[providers.fallback]
driver = "openai_compatible"
base_url = "http://{address}/{fallback_path}/v1"
api_key_env = "PROFILE_SWITCH_FALLBACK_KEY"

[models.primary]
provider = "primary"
model = "primary-model"
[models.primary.capabilities]
supports_images = false
supports_tools = false
supports_system_prompt = true
structured_output = "json_schema"
context_window_tokens = 4096
max_output_tokens = 256
request_timeout_sec = 5
thinking = "none"

[models.fallback]
provider = "fallback"
model = "fallback-model"
[models.fallback.capabilities]
supports_images = false
supports_tools = false
supports_system_prompt = true
structured_output = "json_schema"
context_window_tokens = 4096
max_output_tokens = 256
request_timeout_sec = 5
thinking = "none"

[routes.switch]
models = ["primary", "fallback"]
fallback_on_validation_failure = {fallback_on_validation_failure}
"#
        ))
        .unwrap()
    }

    fn profile_options<'a>(validator: Option<&'a OutputValidator>) -> GenerateTextOptions<'a> {
        GenerateTextOptions {
            route: "switch",
            system_prompt: Some("system"),
            prompt: "prompt",
            image_base64: None,
            temperature: 0.0,
            num_predict: 64,
            output_validator: validator,
            structured_output: None,
        }
    }

    fn completion(content: &str) -> Value {
        json!({
            "id": "switch-test",
            "object": "chat.completion",
            "created": 0,
            "model": "profile-model",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop"
            }]
        })
    }

    #[tokio::test]
    async fn profile_transport_failure_falls_back_and_records_attempts() {
        let _env_lock = PROFILE_ENV_LOCK.lock().await;
        async fn primary() -> impl IntoResponse {
            (StatusCode::INTERNAL_SERVER_ERROR, "unavailable")
        }
        async fn fallback() -> Json<Value> {
            Json(completion("fallback response"))
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/primary/v1/chat/completions", post(primary))
                    .route("/fallback/v1/chat/completions", post(fallback)),
            )
            .await
            .unwrap();
        });
        unsafe {
            std::env::set_var("PROFILE_SWITCH_PRIMARY_KEY", "test-key");
            std::env::set_var("PROFILE_SWITCH_FALLBACK_KEY", "test-key");
        }
        let mut config = config();
        config.llm_profiles = Some(switching_profiles(address, "primary", "fallback", false));

        let generated = generate_text_checked(&config, profile_options(None))
            .await
            .unwrap();

        assert_eq!(generated.content, "fallback response");
        assert_eq!(generated.attempts.len(), 2);
        assert_eq!(generated.attempts[0].provider, "primary");
        assert_eq!(generated.attempts[0].outcome, "http_5xx");
        assert_eq!(generated.attempts[1].provider, "fallback");
        assert_eq!(generated.attempts[1].outcome, "success");
        unsafe {
            std::env::remove_var("PROFILE_SWITCH_PRIMARY_KEY");
            std::env::remove_var("PROFILE_SWITCH_FALLBACK_KEY");
        }
        server.abort();
    }

    #[tokio::test]
    async fn profile_validation_fallback_obeys_route_flag() {
        let _env_lock = PROFILE_ENV_LOCK.lock().await;
        async fn invalid() -> Json<Value> {
            Json(completion("invalid"))
        }
        async fn valid() -> Json<Value> {
            Json(completion("valid"))
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/invalid/v1/chat/completions", post(invalid))
                    .route("/valid/v1/chat/completions", post(valid)),
            )
            .await
            .unwrap();
        });
        unsafe {
            std::env::set_var("PROFILE_SWITCH_PRIMARY_KEY", "test-key");
            std::env::set_var("PROFILE_SWITCH_FALLBACK_KEY", "test-key");
        }
        let validator = |content: &str| -> anyhow::Result<()> {
            anyhow::ensure!(content == "valid", "invalid output");
            Ok(())
        };
        let mut config = config();
        config.llm_profiles = Some(switching_profiles(address, "invalid", "valid", false));
        assert!(
            generate_text_checked(&config, profile_options(Some(&validator)))
                .await
                .is_err()
        );

        config.llm_profiles = Some(switching_profiles(address, "invalid", "valid", true));
        let generated = generate_text_checked(&config, profile_options(Some(&validator)))
            .await
            .unwrap();
        assert_eq!(generated.content, "valid");
        assert_eq!(generated.attempts.len(), 3);
        assert_eq!(generated.attempts[0].outcome, "validation_failed");
        assert_eq!(generated.attempts[1].outcome, "validation_failed");
        assert_eq!(generated.attempts[2].provider, "fallback");
        unsafe {
            std::env::remove_var("PROFILE_SWITCH_PRIMARY_KEY");
            std::env::remove_var("PROFILE_SWITCH_FALLBACK_KEY");
        }
        server.abort();
    }

    #[test]
    fn classifies_typed_empty_responses_for_retry() {
        let empty = anyhow::Error::new(LlmTransportError::empty_response());
        assert!(is_empty_response(&empty));
        assert_eq!(classify_attempt_error(&empty), "empty_response");
    }
}
