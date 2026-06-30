use crate::config::Config;
use crate::llm::ollama::OllamaClient;
use crate::llm::openai_compat::OpenAiCompatClient;
use crate::llm::types::{GeneratedText, LlmClient, LlmRequest};

const GROQ_OPENAI_BASE_URL: &str = "https://api.groq.com/openai/v1";
const OPENROUTER_OPENAI_BASE_URL: &str = "https://openrouter.ai/api/v1";

pub async fn generate_text(
    config: &Config,
    prompt: &str,
    image_base64: Option<&str>,
    temperature: f32,
    num_predict: u32,
) -> anyhow::Result<GeneratedText> {
    let provider = normalize_provider(&config.llm_provider)?;
    let model = model_for_provider(config, provider);
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
        "openrouter" => {
            OpenAiCompatClient::new(OPENROUTER_OPENAI_BASE_URL, &config.openrouter_api_key)
                .generate(request)
                .await?
        }
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

fn normalize_provider(provider: &str) -> anyhow::Result<&'static str> {
    match provider.trim().to_lowercase().as_str() {
        "" | "ollama" => Ok("ollama"),
        "groq" => Ok("groq"),
        "openrouter" => Ok("openrouter"),
        "openai_compat" => Ok("openai_compat"),
        other => anyhow::bail!("unknown LLM_PROVIDER: {other}"),
    }
}

fn model_for_provider<'a>(config: &'a Config, provider: &str) -> &'a str {
    match provider {
        "groq" | "openrouter" => config.llm_model.as_deref().unwrap_or(&config.vision_model),
        "openai_compat" => config
            .openai_compat_model
            .as_deref()
            .or(config.llm_model.as_deref())
            .unwrap_or(&config.vision_model),
        _ => &config.vision_model,
    }
}

fn supports_images(config: &Config, provider: &str, model: &str) -> bool {
    if let Some(supports_images) = config.llm_supports_images {
        return supports_images;
    }

    let model = model.to_lowercase();
    matches!(provider, "ollama")
        || model.contains("vision")
        || model.contains("llama-4")
        || model.contains("gpt-4o")
        || model.contains("gemma4")
        || model.contains("gemini")
        || model.contains("pixtral")
}
