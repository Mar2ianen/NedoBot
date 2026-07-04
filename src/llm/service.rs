use crate::config::{Config, normalize_llm_provider};
use crate::llm::gemini::GeminiClient;
use crate::llm::ollama::OllamaClient;
use crate::llm::openai_compat::OpenAiCompatClient;
use crate::llm::types::{GeneratedText, LlmClient, LlmRequest};

pub type OutputValidator = fn(&str) -> anyhow::Result<()>;

pub struct GenerateTextOptions<'a> {
    pub provider_override: Option<&'a str>,
    pub model_override: Option<&'a str>,
    pub prompt: &'a str,
    pub image_base64: Option<&'a str>,
    pub temperature: f32,
    pub num_predict: u32,
    pub output_validator: Option<OutputValidator>,
}

const GROQ_OPENAI_BASE_URL: &str = "https://api.groq.com/openai/v1";
const CEREBRAS_OPENAI_BASE_URL: &str = "https://api.cerebras.ai/v1";
const OPENROUTER_OPENAI_BASE_URL: &str = "https://openrouter.ai/api/v1";

pub async fn generate_text(
    config: &Config,
    prompt: &str,
    image_base64: Option<&str>,
    temperature: f32,
    num_predict: u32,
) -> anyhow::Result<GeneratedText> {
    generate_text_checked(config, prompt, image_base64, temperature, num_predict, None).await
}

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
    generate_text_with_provider_checked(
        config,
        GenerateTextOptions {
            provider_override,
            model_override,
            prompt,
            image_base64,
            temperature,
            num_predict,
            output_validator: None,
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

    for fallback in fallbacks {
        match generate_once(
            config,
            fallback.provider,
            fallback.model,
            options.prompt,
            options.image_base64,
            options.temperature,
            options.num_predict,
        )
        .await
        {
            Ok(generation) => {
                if let Some(validate) = options.output_validator
                    && let Err(err) = validate(&generation.content)
                {
                    tracing::warn!(%err, provider = fallback.provider, model = fallback.model, "LLM generation output failed validation");
                    last_error = Some(err);
                    continue;
                }
                return Ok(generation);
            }
            Err(err) => {
                tracing::warn!(%err, provider = fallback.provider, model = fallback.model, "LLM generation attempt failed");
                last_error = Some(err);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no LLM generation attempts were configured")))
}

async fn generate_once(
    config: &Config,
    provider: &str,
    model: &str,
    prompt: &str,
    image_base64: Option<&str>,
    temperature: f32,
    num_predict: u32,
) -> anyhow::Result<GeneratedText> {
    let image_base64 = image_base64.filter(|_| supports_images(config, provider, model));
    let request = LlmRequest {
        model,
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
            comment_custom_emoji_id: None,
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
