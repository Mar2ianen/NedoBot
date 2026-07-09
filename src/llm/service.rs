use crate::config::{Config, normalize_llm_provider};
use crate::llm::gemini::GeminiClient;
use crate::llm::ollama::OllamaClient;
use crate::llm::openai_compat::OpenAiCompatClient;
use crate::llm::types::{GeneratedText, LlmAttempt, LlmClient, LlmRequest};

pub type OutputValidator = fn(&str) -> anyhow::Result<()>;

pub struct GenerateTextOptions<'a> {
    pub provider_override: Option<&'a str>,
    pub model_override: Option<&'a str>,
    pub system_prompt: Option<&'a str>,
    pub prompt: &'a str,
    pub image_base64: Option<&'a str>,
    pub temperature: f32,
    pub num_predict: u32,
    pub output_validator: Option<OutputValidator>,
}

const GROQ_OPENAI_BASE_URL: &str = "https://api.groq.com/openai/v1";
const CEREBRAS_OPENAI_BASE_URL: &str = "https://api.cerebras.ai/v1";
const OPENROUTER_OPENAI_BASE_URL: &str = "https://openrouter.ai/api/v1";
const VALIDATION_RETRY_ATTEMPTS: usize = 1;

#[allow(dead_code)]
pub async fn generate_text(
    config: &Config,
    prompt: &str,
    image_base64: Option<&str>,
    temperature: f32,
    num_predict: u32,
) -> anyhow::Result<GeneratedText> {
    generate_text_checked(config, prompt, image_base64, temperature, num_predict, None).await
}

#[allow(dead_code)]
pub async fn generate_text_checked(
    config: &Config,
    prompt: &str,
    image_base64: Option<&str>,
    temperature: f32,
    num_predict: u32,
    output_validator: Option<OutputValidator>,
) -> anyhow::Result<GeneratedText> {
    generate_text_with_provider_checked(
        config,
        GenerateTextOptions {
            provider_override: None,
            model_override: None,
            system_prompt: None,
            prompt,
            image_base64,
            temperature,
            num_predict,
            output_validator,
        },
    )
    .await
}

pub async fn generate_text_with_provider(
    config: &Config,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    prompt: &str,
    image_base64: Option<&str>,
    temperature: f32,
    num_predict: u32,
) -> anyhow::Result<GeneratedText> {
    generate_text_with_provider_and_system(
        config,
        provider_override,
        model_override,
        None,
        prompt,
        image_base64,
        temperature,
        num_predict,
    )
    .await
}

pub async fn generate_text_with_provider_and_system(
    config: &Config,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    system_prompt: Option<&str>,
    prompt: &str,
    image_base64: Option<&str>,
    temperature: f32,
    num_predict: u32,
) -> anyhow::Result<GeneratedText> {
    generate_text_with_provider_checked(
        config,
        GenerateTextOptions {
            provider_override,
            model_override,
            system_prompt,
            prompt,
            image_base64,
            temperature,
            num_predict,
            output_validator: None,
        },
    )
    .await
}

pub async fn generate_text_checked_with_system(
    config: &Config,
    system_prompt: &str,
    prompt: &str,
    image_base64: Option<&str>,
    temperature: f32,
    num_predict: u32,
    output_validator: Option<OutputValidator>,
) -> anyhow::Result<GeneratedText> {
    generate_text_with_provider_checked(
        config,
        GenerateTextOptions {
            provider_override: None,
            model_override: None,
            system_prompt: Some(system_prompt),
            prompt,
            image_base64,
            temperature,
            num_predict,
            output_validator,
        },
    )
    .await
}

pub async fn generate_text_with_provider_checked(
    config: &Config,
    options: GenerateTextOptions<'_>,
) -> anyhow::Result<GeneratedText> {
    let provider =
        normalize_llm_provider(options.provider_override.unwrap_or(&config.llm_provider))?;
    let model = match options.model_override {
        Some(model) => model,
        None => model_for_provider(config, provider)?,
    };

    let fallbacks = fallback_models(config, provider, options.model_override, model);
    let mut last_error = None;
    let mut attempts = Vec::new();

    for (fallback_index, fallback) in fallbacks.into_iter().enumerate() {
        let mut attempt_prompt = options.prompt.to_string();
        for attempt in 0..=VALIDATION_RETRY_ATTEMPTS {
            match generate_once(
                config,
                fallback.provider,
                fallback.model,
                options.system_prompt,
                &attempt_prompt,
                options.image_base64,
                options.temperature,
                options.num_predict,
            )
            .await
            {
                Ok(mut generation) => {
                    let llm_attempt = generation.attempts.pop().unwrap_or_else(|| LlmAttempt {
                        provider: fallback.provider.to_string(),
                        model: fallback.model.to_string(),
                        outcome: "success".to_string(),
                    });
                    if fallback_index > 0 {
                        tracing::info!(
                            fallback_index,
                            provider = fallback.provider,
                            model = fallback.model,
                            "LLM fallback succeeded"
                        );
                    }
                    if let Some(validate) = options.output_validator
                        && let Err(err) = validate(&generation.content)
                    {
                        tracing::warn!(
                            %err,
                            fallback_index,
                            is_fallback = fallback_index > 0,
                            provider = fallback.provider,
                            model = fallback.model,
                            attempt,
                            "LLM generation output failed validation"
                        );
                        attempts.push(LlmAttempt {
                            outcome: "validation_failed".to_string(),
                            ..llm_attempt
                        });
                        last_error = Some(err);
                        if attempt < VALIDATION_RETRY_ATTEMPTS {
                            attempt_prompt = validation_retry_prompt(
                                options.prompt,
                                &format!("{:#}", last_error.as_ref().unwrap()),
                            );
                            continue;
                        }
                        break;
                    }
                    attempts.push(llm_attempt);
                    generation.attempts = attempts;
                    return Ok(generation);
                }
                Err(err) => {
                    tracing::warn!(
                        %err,
                        fallback_index,
                        is_fallback = fallback_index > 0,
                        provider = fallback.provider,
                        model = fallback.model,
                        attempt,
                        "LLM generation attempt failed"
                    );
                    attempts.push(LlmAttempt {
                        provider: fallback.provider.to_string(),
                        model: fallback.model.to_string(),
                        outcome: classify_attempt_error(&err),
                    });
                    last_error = Some(err);
                    break;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no LLM generation attempts were configured")))
}

fn classify_attempt_error(error: &anyhow::Error) -> String {
    let text = error.to_string().to_lowercase();
    if text.contains("429") {
        "http_429".to_string()
    } else if text.contains("503") || text.contains("502") || text.contains("500") {
        "http_5xx".to_string()
    } else if text.contains("timeout") || text.contains("timed out") {
        "timeout".to_string()
    } else {
        "error".to_string()
    }
}

async fn generate_once(
    config: &Config,
    provider: &str,
    model: &str,
    system_prompt: Option<&str>,
    prompt: &str,
    image_base64: Option<&str>,
    temperature: f32,
    num_predict: u32,
) -> anyhow::Result<GeneratedText> {
    let image_base64 = image_base64.filter(|_| supports_images(config, provider, model));
    let request = LlmRequest {
        model,
        system_prompt,
        prompt,
        image_base64,
        temperature,
        num_predict,
    };
    let response = match provider {
        "groq" => {
            OpenAiCompatClient::new(GROQ_OPENAI_BASE_URL, &config.groq_api_key)
                .generate(request)
                .await?
        }
        "cerebras" => {
            OpenAiCompatClient::new(CEREBRAS_OPENAI_BASE_URL, &config.cerebras_api_key)
                .generate(request)
                .await?
        }
        "openrouter" => {
            OpenAiCompatClient::new(OPENROUTER_OPENAI_BASE_URL, &config.openrouter_api_key)
                .generate(request)
                .await?
        }
        "gemini" => GeminiClient::new(config).generate(request).await?,
        "openai_compat" => {
            OpenAiCompatClient::from_config(config)
                .generate(request)
                .await?
        }
        _ => OllamaClient::new(config).generate(request).await?,
    };

    Ok(GeneratedText {
        provider: provider.to_string(),
        model: model.to_string(),
        content: response.content,
        image_used: image_base64.is_some(),
        attempts: vec![LlmAttempt {
            provider: provider.to_string(),
            model: model.to_string(),
            outcome: "success".to_string(),
        }],
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FallbackModel<'a> {
    provider: &'static str,
    model: &'a str,
}

fn fallback_models<'a>(
    config: &'a Config,
    provider: &'static str,
    model_override: Option<&str>,
    model: &'a str,
) -> Vec<FallbackModel<'a>> {
    let primary = FallbackModel { provider, model };
    match (provider, model_override) {
        ("gemini", None) => {
            let mut models = vec![primary];
            push_unique_model(&mut models, "gemini", config.gemini_flash_model.trim());
            push_unique_model(&mut models, "ollama", config.vision_model.trim());
            models
        }
        _ => vec![primary],
    }
}

fn validation_retry_prompt(original_prompt: &str, validation_error: &str) -> String {
    format!(
        "{original_prompt}\n\nПредыдущий ответ не прошёл автоматическую проверку: {validation_error}. Верни новый ответ, строго соблюдая формат, ограничения длины и обязательные токены из системных правил."
    )
}

fn push_unique_model<'a>(
    models: &mut Vec<FallbackModel<'a>>,
    provider: &'static str,
    model: &'a str,
) {
    match model.is_empty()
        || models
            .iter()
            .any(|item| item.provider == provider && item.model.eq_ignore_ascii_case(model))
    {
        true => {}
        false => models.push(FallbackModel { provider, model }),
    }
}

fn model_for_provider<'a>(config: &'a Config, provider: &str) -> anyhow::Result<&'a str> {
    match provider {
        "groq" => config
            .llm_model
            .as_deref()
            .or(config.groq_model.as_deref())
            .ok_or_else(|| anyhow::anyhow!("LLM_PROVIDER=groq requires LLM_MODEL or GROQ_MODEL")),
        "cerebras" => config
            .llm_model
            .as_deref()
            .or(config.cerebras_model.as_deref())
            .ok_or_else(|| {
                anyhow::anyhow!("LLM_PROVIDER=cerebras requires LLM_MODEL or CEREBRAS_MODEL")
            }),
        "openrouter" => config
            .llm_model
            .as_deref()
            .or(config.openrouter_model.as_deref())
            .ok_or_else(|| {
                anyhow::anyhow!("LLM_PROVIDER=openrouter requires LLM_MODEL or OPENROUTER_MODEL")
            }),
        "gemini" => Ok(config
            .llm_model
            .as_deref()
            .unwrap_or(&config.gemini_text_model)),
        "openai_compat" => Ok(config
            .openai_compat_model
            .as_deref()
            .or(config.llm_model.as_deref())
            .unwrap_or(&config.vision_model)),
        _ => Ok(&config.vision_model),
    }
}

fn supports_images(config: &Config, provider: &str, model: &str) -> bool {
    if let Some(supports_images) = config.llm_supports_images {
        return supports_images;
    }

    let model = model.to_lowercase();
    matches!(provider, "ollama" | "gemini")
        || model.contains("vision")
        || model.contains("llama-4")
        || model.contains("gpt-4o")
        || model.contains("gemma4")
        || model.contains("gemini")
        || model.contains("pixtral")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SearchMcpTools;

    fn config() -> Config {
        Config {
            source_channel_id: -1001,
            discussion_chat_id: -1002,
            chat_invite_url: "https://t.me/example".to_string(),
            chat_invite_label: "чат".to_string(),
            post_signature_marker: "marker".to_string(),
            llm_provider: "gemini".to_string(),
            llm_model: Some("gemini-3.5-flash".to_string()),
            llm_supports_images: Some(true),
            llm_temperature: 0.35,
            llm_max_tokens: 90,
            llm_proxy_url: None,
            memory_llm_temperature: 0.2,
            memory_llm_max_tokens: 220,
            search_enabled: false,
            search_extract_provider: Some("ollama".to_string()),
            search_extract_model: Some("gemma4:31b".to_string()),
            search_extract_temperature: 0.1,
            search_extract_max_tokens: 700,
            search_mcp_command: None,
            search_mcp_args: Vec::new(),
            search_mcp_env: Vec::new(),
            search_mcp_timeout_sec: 8,
            search_mcp_tools: SearchMcpTools {
                web: "web_search".to_string(),
                github: "github_search".to_string(),
                reddit: "reddit_search".to_string(),
            },
            search_mcp_fetch_tool: Some("web_fetch_exa".to_string()),
            search_fetch_top_n: 2,
            search_fetch_max_chars: 6000,
            search_github_mcp_command: None,
            search_github_mcp_args: Vec::new(),
            search_github_mcp_env: vec![
                "PATH".to_string(),
                "HOME".to_string(),
                "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
            ],
            search_github_mcp_tools: vec!["search_issues".to_string(), "search_code".to_string()],
            groq_api_key: String::new(),
            groq_model: None,
            cerebras_api_key: String::new(),
            cerebras_model: None,
            openrouter_api_key: String::new(),
            openrouter_model: None,
            gemini_api_key: String::new(),
            gemini_text_model: "gemini-3.5-flash".to_string(),
            gemini_flash_model: "gemini-3.1-flash-lite".to_string(),
            gemini_tts_model: "gemini-3.1-flash-tts-preview".to_string(),
            gemini_thinking_budget: 1024,
            ollama_base_url: "https://ollama.com".to_string(),
            ollama_api_key: String::new(),
            openai_compat_base_url: "https://api.openai.com/v1".to_string(),
            openai_compat_api_key: String::new(),
            openai_compat_model: None,
            vision_model: "gemma4:31b".to_string(),
            owner_telegram_id: None,
            send_owner_preview: false,
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
            voice_cleanup_provider: None,
            voice_cleanup_model: None,
            voice_cleanup_temperature: 0.2,
            voice_cleanup_max_tokens: 1800,
            voice_render_expandable_chapters: true,
            voice_send_full_file: true,
            public_base_url: None,
            static_files_dir: "/tmp/tg-ai-bot-static".to_string(),
        }
    }

    #[test]
    fn gemini_comments_fallback_to_flash_lite_then_gemma_31b() {
        let config = config();
        let models = fallback_models(&config, "gemini", None, "gemini-3.5-flash");

        assert_eq!(
            models,
            vec![
                FallbackModel {
                    provider: "gemini",
                    model: "gemini-3.5-flash",
                },
                FallbackModel {
                    provider: "gemini",
                    model: "gemini-3.1-flash-lite",
                },
                FallbackModel {
                    provider: "ollama",
                    model: "gemma4:31b",
                },
            ]
        );
    }

    #[test]
    fn explicit_model_override_disables_comment_fallback_chain() {
        let config = config();
        let models = fallback_models(
            &config,
            "gemini",
            Some("gemini-3.5-flash"),
            "gemini-3.5-flash",
        );

        assert_eq!(
            models,
            vec![FallbackModel {
                provider: "gemini",
                model: "gemini-3.5-flash",
            }]
        );
    }
}
