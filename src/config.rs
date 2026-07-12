#[derive(Clone)]
#[allow(dead_code)]
pub struct SearchMcpTools {
    pub web: String,
    pub github: String,
    pub reddit: String,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct Config {
    pub source_channel_id: i64,
    pub discussion_chat_id: i64,
    pub chat_invite_url: String,
    pub chat_invite_label: String,
    pub post_signature_marker: String,
    pub llm_provider: String,
    pub llm_model: Option<String>,
    pub llm_supports_images: Option<bool>,
    pub llm_temperature: f32,
    pub llm_max_tokens: u32,
    pub llm_proxy_url: Option<String>,
    pub memory_llm_temperature: f32,
    pub memory_llm_max_tokens: u32,
    pub search_enabled: bool,
    pub search_extract_provider: Option<String>,
    pub search_extract_model: Option<String>,
    pub search_extract_temperature: f32,
    pub search_extract_max_tokens: u32,
    pub search_mcp_command: Option<String>,
    pub search_mcp_args: Vec<String>,
    pub search_mcp_env: Vec<String>,
    pub search_mcp_timeout_sec: u64,
    pub search_mcp_tools: SearchMcpTools,
    pub search_mcp_fetch_tool: Option<String>,
    pub search_fetch_top_n: usize,
    pub search_fetch_max_chars: usize,
    pub search_github_mcp_command: Option<String>,
    pub search_github_mcp_args: Vec<String>,
    pub search_github_mcp_env: Vec<String>,
    pub search_github_mcp_tools: Vec<String>,
    pub groq_api_key: String,
    pub groq_model: Option<String>,
    pub cerebras_api_key: String,
    pub cerebras_model: Option<String>,
    pub openrouter_api_key: String,
    pub openrouter_model: Option<String>,
    pub gemini_api_key: String,
    pub gemini_text_model: String,
    pub gemini_flash_model: String,
    pub gemini_tts_model: String,
    pub gemini_thinking_budget: u32,
    pub ollama_base_url: String,
    pub ollama_api_key: String,
    pub openai_compat_base_url: String,
    pub openai_compat_api_key: String,
    pub openai_compat_model: Option<String>,
    pub vision_model: String,
    pub owner_telegram_id: Option<i64>,
    pub send_owner_preview: bool,
    pub profile_refresh_concurrency: usize,
    pub comment_custom_emoji_id: Option<String>,
    pub first_comment_max_image_mb: u32,
    pub tech_custom_emoji_id: Option<String>,
    pub amd_custom_emoji_id: Option<String>,
    pub radeon_custom_emoji_id: Option<String>,
    pub ryzen_custom_emoji_id: Option<String>,
    pub voice_transcription_enabled: bool,
    pub voice_auto_transcribe: bool,
    pub voice_max_duration_sec: u32,
    pub voice_max_file_mb: u32,
    pub voice_short_text_max_chars: usize,
    pub voice_language: String,
    pub voice_asr_provider: String,
    pub voice_asr_model: String,
    pub voice_asr_temperature: f32,
    pub voice_cleanup_provider: Option<String>,
    pub voice_cleanup_model: Option<String>,
    pub voice_cleanup_temperature: f32,
    pub voice_cleanup_max_tokens: u32,
    pub voice_render_expandable_chapters: bool,
    pub voice_send_full_file: bool,
    pub public_base_url: Option<String>,
    pub static_files_dir: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            source_channel_id: env_i64("SOURCE_CHANNEL_ID", -1001575496091),
            discussion_chat_id: env_i64("DISCUSSION_CHAT_ID", -1001932061163),
            chat_invite_url: env_or("CHAT_INVITE_URL", "https://t.me/+RxmPtw7Bs-IxNzEy"),
            chat_invite_label: env_or("CHAT_INVITE_LABEL", "Присоединяйтесь к чату"),
            post_signature_marker: env_or("POST_SIGNATURE_MARKER", "Не теряем связь"),
            llm_provider: env_or("LLM_PROVIDER", "ollama"),
            llm_model: env_optional("LLM_MODEL"),
            llm_supports_images: env_optional("LLM_SUPPORTS_IMAGES")
                .and_then(|value| value.parse().ok()),
            llm_temperature: env_f32("LLM_TEMPERATURE", 0.45),
            llm_max_tokens: env_u32("LLM_MAX_TOKENS", 90),
            llm_proxy_url: env_optional("LLM_PROXY_URL"),
            memory_llm_temperature: env_f32("MEMORY_LLM_TEMPERATURE", 0.2),
            memory_llm_max_tokens: env_u32("MEMORY_LLM_MAX_TOKENS", 220),
            search_enabled: env_bool("SEARCH_ENABLED", false),
            search_extract_provider: env_optional("SEARCH_EXTRACT_PROVIDER")
                .or_else(|| Some("ollama".to_string())),
            search_extract_model: env_optional("SEARCH_EXTRACT_MODEL")
                .or_else(|| Some("gemma4:31b".to_string())),
            search_extract_temperature: env_f32("SEARCH_EXTRACT_TEMPERATURE", 0.1),
            search_extract_max_tokens: env_u32("SEARCH_EXTRACT_MAX_TOKENS", 700),
            search_mcp_command: env_optional("SEARCH_MCP_COMMAND"),
            search_mcp_args: env_args("SEARCH_MCP_ARGS"),
            search_mcp_env: env_list_csv("SEARCH_MCP_ENV"),
            search_mcp_timeout_sec: env_u64("SEARCH_MCP_TIMEOUT_SEC", 8),
            search_mcp_tools: SearchMcpTools {
                web: env_or("SEARCH_MCP_TOOL_WEB", "web_search"),
                github: env_or("SEARCH_MCP_TOOL_GITHUB", "github_search"),
                reddit: env_or("SEARCH_MCP_TOOL_REDDIT", "reddit_search"),
            },
            search_mcp_fetch_tool: env_optional("SEARCH_MCP_TOOL_FETCH")
                .or_else(|| Some("web_fetch_exa".to_string())),
            search_fetch_top_n: env_usize("SEARCH_FETCH_TOP_N", 2),
            search_fetch_max_chars: env_usize("SEARCH_FETCH_MAX_CHARS", 6000),
            search_github_mcp_command: env_optional("SEARCH_GITHUB_MCP_COMMAND"),
            search_github_mcp_args: env_args("SEARCH_GITHUB_MCP_ARGS"),
            search_github_mcp_env: env_list_csv_or(
                "SEARCH_GITHUB_MCP_ENV",
                &["PATH", "HOME", "GITHUB_PERSONAL_ACCESS_TOKEN"],
            ),
            search_github_mcp_tools: env_list_csv_or(
                "SEARCH_GITHUB_MCP_TOOLS",
                &["search_issues", "search_code"],
            ),
            groq_api_key: env_or("GROQ_API_KEY", ""),
            groq_model: env_optional("GROQ_MODEL"),
            cerebras_api_key: env_or("CEREBRAS_API_KEY", ""),
            cerebras_model: env_optional("CEREBRAS_MODEL"),
            openrouter_api_key: env_or("OPENROUTER_API_KEY", ""),
            openrouter_model: env_optional("OPENROUTER_MODEL"),
            gemini_api_key: env_optional("GEMINI_API_KEY")
                .or_else(|| env_optional("GOOGLE_AI_STUDIO_API_KEY"))
                .unwrap_or_default(),
            gemini_text_model: env_or("GEMINI_TEXT_MODEL", "gemini-3.5-flash"),
            gemini_flash_model: env_or("GEMINI_FLASH_MODEL", "gemini-3.1-flash-lite"),
            gemini_tts_model: env_or("GEMINI_TTS_MODEL", "gemini-3.1-flash-tts-preview"),
            gemini_thinking_budget: env_u32("GEMINI_THINKING_BUDGET", 1024),
            ollama_base_url: env_or("OLLAMA_BASE_URL", "https://ollama.com"),
            ollama_api_key: env_or("OLLAMA_API_KEY", ""),
            openai_compat_base_url: env_or("OPENAI_COMPAT_BASE_URL", "https://api.openai.com/v1"),
            openai_compat_api_key: env_or("OPENAI_COMPAT_API_KEY", ""),
            openai_compat_model: env_optional("OPENAI_COMPAT_MODEL"),
            vision_model: env_optional("VISION_MODEL")
                .or_else(|| env_optional("OLLAMA_MODEL"))
                .unwrap_or_else(|| "gemma4:31b".to_string()),
            owner_telegram_id: env_optional("OWNER_TELEGRAM_ID")
                .and_then(|value| value.parse().ok()),
            send_owner_preview: env_or("SEND_OWNER_PREVIEW", "true") == "true",
            profile_refresh_concurrency: env_usize("PROFILE_REFRESH_CONCURRENCY", 4),
            comment_custom_emoji_id: env_optional("COMMENT_CUSTOM_EMOJI_ID"),
            first_comment_max_image_mb: env_u32("FIRST_COMMENT_MAX_IMAGE_MB", 10),
            tech_custom_emoji_id: env_optional("TECH_CUSTOM_EMOJI_ID"),
            amd_custom_emoji_id: env_optional("AMD_CUSTOM_EMOJI_ID"),
            radeon_custom_emoji_id: env_optional("RADEON_CUSTOM_EMOJI_ID"),
            ryzen_custom_emoji_id: env_optional("RYZEN_CUSTOM_EMOJI_ID"),
            voice_transcription_enabled: env_bool("VOICE_TRANSCRIPTION_ENABLED", false),
            voice_auto_transcribe: env_bool("VOICE_AUTO_TRANSCRIBE", false),
            voice_max_duration_sec: env_u32("VOICE_MAX_DURATION_SEC", 600),
            voice_max_file_mb: env_u32("VOICE_MAX_FILE_MB", 20),
            voice_short_text_max_chars: env_usize("VOICE_SHORT_TEXT_MAX_CHARS", 400),
            voice_language: env_or("VOICE_LANGUAGE", "ru"),
            voice_asr_provider: env_or("VOICE_ASR_PROVIDER", "groq"),
            voice_asr_model: env_or("VOICE_ASR_MODEL", "whisper-large-v3"),
            voice_asr_temperature: env_f32("VOICE_ASR_TEMPERATURE", 0.0),
            voice_cleanup_provider: env_optional("VOICE_CLEANUP_PROVIDER"),
            voice_cleanup_model: env_optional("VOICE_CLEANUP_MODEL"),
            voice_cleanup_temperature: env_f32("VOICE_CLEANUP_TEMPERATURE", 0.2),
            voice_cleanup_max_tokens: env_u32("VOICE_CLEANUP_MAX_TOKENS", 1800),
            voice_render_expandable_chapters: env_bool("VOICE_RENDER_EXPANDABLE_CHAPTERS", true),
            voice_send_full_file: env_bool("VOICE_SEND_FULL_FILE", true),
            public_base_url: env_optional("PUBLIC_BASE_URL"),
            static_files_dir: env_or("STATIC_FILES_DIR", "/opt/tg-ai-bot-teloxide/static"),
        }
    }

    pub fn validate_runtime_secrets(&self) -> anyhow::Result<()> {
        let mut errors = Vec::new();

        validate_llm_provider_secret(&mut errors, self, &self.llm_provider, "LLM_PROVIDER");
        validate_llm_provider_model(&mut errors, self, &self.llm_provider, "LLM_PROVIDER");

        if self.voice_transcription_enabled && self.voice_auto_transcribe {
            validate_voice_asr_secret(&mut errors, self);
            if let Some(provider) = self.voice_cleanup_provider.as_deref() {
                validate_llm_provider_secret(&mut errors, self, provider, "VOICE_CLEANUP_PROVIDER");
                validate_llm_provider_model(&mut errors, self, provider, "VOICE_CLEANUP_PROVIDER");
            }
        }

        if self.search_enabled {
            validate_search_config(&mut errors, self);
            if let Some(provider) = self.search_extract_provider.as_deref() {
                validate_llm_provider_secret(
                    &mut errors,
                    self,
                    provider,
                    "SEARCH_EXTRACT_PROVIDER",
                );
                validate_llm_provider_model_with_model(
                    &mut errors,
                    self,
                    provider,
                    "SEARCH_EXTRACT_PROVIDER",
                    self.search_extract_model.as_deref(),
                    "SEARCH_EXTRACT_MODEL",
                );
            }
        }

        if self.profile_refresh_concurrency == 0 {
            errors.push("PROFILE_REFRESH_CONCURRENCY must be greater than 0".to_string());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!(
                "invalid runtime secret configuration:\n- {}",
                errors.join("\n- ")
            )
        }
    }
}

fn validate_llm_provider_model(
    errors: &mut Vec<String>,
    config: &Config,
    provider: &str,
    context: &str,
) {
    validate_llm_provider_model_with_model(
        errors,
        config,
        provider,
        context,
        config.llm_model.as_deref(),
        "LLM_MODEL",
    );
}

fn validate_llm_provider_model_with_model(
    errors: &mut Vec<String>,
    config: &Config,
    provider: &str,
    context: &str,
    model: Option<&str>,
    model_key: &str,
) {
    match normalize_llm_provider(provider) {
        Ok("groq") if model.is_none() && config.groq_model.is_none() => errors.push(format!(
            "{context}=groq requires {model_key} or GROQ_MODEL; refusing to fallback to VISION_MODEL"
        )),
        Ok("cerebras") if model.is_none() && config.cerebras_model.is_none() => {
            errors.push(format!(
                "{context}=cerebras requires {model_key} or CEREBRAS_MODEL; refusing to fallback to VISION_MODEL"
            ));
        }
        Ok("openrouter") if model.is_none() && config.openrouter_model.is_none() => {
            errors.push(format!(
                "{context}=openrouter requires {model_key} or OPENROUTER_MODEL; refusing to fallback to VISION_MODEL"
            ));
        }
        Ok(_) | Err(_) => {}
    }
}

fn validate_search_config(errors: &mut Vec<String>, config: &Config) {
    if config.search_mcp_command.is_none() {
        errors.push("SEARCH_ENABLED=true requires non-empty SEARCH_MCP_COMMAND".to_string());
    }

    if config.search_mcp_timeout_sec == 0 {
        errors.push("SEARCH_MCP_TIMEOUT_SEC must be greater than 0".to_string());
    }

    if config.search_fetch_max_chars == 0 {
        errors.push("SEARCH_FETCH_MAX_CHARS must be greater than 0".to_string());
    }
}

fn validate_voice_asr_secret(errors: &mut Vec<String>, config: &Config) {
    match config.voice_asr_provider.trim().to_lowercase().as_str() {
        "groq" => require_secret(
            errors,
            "GROQ_API_KEY",
            &config.groq_api_key,
            "VOICE_ASR_PROVIDER=groq",
        ),
        provider => errors.push(format!(
            "VOICE_ASR_PROVIDER={provider} is unsupported; supported provider: groq"
        )),
    }
}

fn validate_llm_provider_secret(
    errors: &mut Vec<String>,
    config: &Config,
    provider: &str,
    context: &str,
) {
    match normalize_llm_provider(provider) {
        Ok("ollama") => {}
        Ok("groq") => require_secret(errors, "GROQ_API_KEY", &config.groq_api_key, context),
        Ok("cerebras") => require_secret(
            errors,
            "CEREBRAS_API_KEY",
            &config.cerebras_api_key,
            context,
        ),
        Ok("openrouter") => require_secret(
            errors,
            "OPENROUTER_API_KEY",
            &config.openrouter_api_key,
            context,
        ),
        Ok("gemini") => require_secret(
            errors,
            "GEMINI_API_KEY or GOOGLE_AI_STUDIO_API_KEY",
            &config.gemini_api_key,
            context,
        ),
        Ok("openai_compat") => require_secret(
            errors,
            "OPENAI_COMPAT_API_KEY",
            &config.openai_compat_api_key,
            context,
        ),
        Ok(_) => unreachable!("all normalized providers are matched"),
        Err(err) => errors.push(format!(
            "{context} has unsupported provider {provider:?}: {err}"
        )),
    }
}

pub(crate) fn normalize_llm_provider(provider: &str) -> anyhow::Result<&'static str> {
    match provider.trim().to_lowercase().as_str() {
        "" | "ollama" => Ok("ollama"),
        "groq" => Ok("groq"),
        "cerebras" => Ok("cerebras"),
        "openrouter" => Ok("openrouter"),
        "gemini" | "google" | "google_ai_studio" => Ok("gemini"),
        "openai_compat" => Ok("openai_compat"),
        other => anyhow::bail!("unknown provider: {other}"),
    }
}

fn require_secret(errors: &mut Vec<String>, key: &str, value: &str, context: &str) {
    if value.trim().is_empty() {
        errors.push(format!("{context} requires non-empty {key}"));
    }
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_i64(key: &str, default: i64) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_optional(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_list_csv(name: &str) -> Vec<String> {
    parse_csv_env(name).unwrap_or_default()
}

fn env_list_csv_or(name: &str, default: &[&str]) -> Vec<String> {
    parse_csv_env(name).unwrap_or_else(|| default.iter().map(ToString::to_string).collect())
}

fn parse_csv_env(name: &str) -> Option<Vec<String>> {
    std::env::var(name).ok().map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(ToString::to_string)
            .collect()
    })
}

fn env_args(name: &str) -> Vec<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.split_whitespace().map(ToString::to_string).collect())
        .unwrap_or_default()
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
            llm_provider: "ollama".to_string(),
            llm_model: Some("gemma4:31b".to_string()),
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
    fn gemini_provider_requires_gemini_key_at_startup() {
        let mut config = config();
        config.llm_provider = "gemini".to_string();
        config.llm_model = Some("gemini-3.5-flash".to_string());

        let err = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(err.contains("LLM_PROVIDER requires non-empty GEMINI_API_KEY"));
    }

    #[test]
    fn groq_provider_requires_explicit_model_at_startup() {
        let mut config = config();
        config.llm_provider = "groq".to_string();
        config.llm_model = None;
        config.groq_model = None;
        config.groq_api_key = "secret".to_string();

        let err = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(err.contains("LLM_PROVIDER=groq requires LLM_MODEL or GROQ_MODEL"));
    }

    #[test]
    fn enabled_voice_pipeline_requires_asr_key() {
        let mut config = config();
        config.voice_transcription_enabled = true;
        config.voice_auto_transcribe = true;

        let err = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(err.contains("VOICE_ASR_PROVIDER=groq requires non-empty GROQ_API_KEY"));
    }

    #[test]
    fn configured_voice_cleanup_provider_requires_its_key() {
        let mut config = config();
        config.voice_transcription_enabled = true;
        config.voice_auto_transcribe = true;
        config.groq_api_key = "groq-key".to_string();
        config.voice_cleanup_provider = Some("openrouter".to_string());

        let err = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(err.contains("VOICE_CLEANUP_PROVIDER requires non-empty OPENROUTER_API_KEY"));
    }

    #[test]
    fn ollama_without_secrets_is_valid_when_voice_is_disabled() {
        let config = config();

        config.validate_runtime_secrets().unwrap();
    }

    #[test]
    fn disabled_search_does_not_validate_mcp_command() {
        let mut config = config();
        config.search_enabled = false;
        config.search_mcp_command = None;
        config.search_mcp_timeout_sec = 0;

        config.validate_runtime_secrets().unwrap();
    }

    #[test]
    fn enabled_search_requires_mcp_command_and_timeout() {
        let mut config = config();
        config.search_enabled = true;
        config.search_mcp_command = None;
        config.search_mcp_timeout_sec = 0;

        let err = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(err.contains("SEARCH_ENABLED=true requires non-empty SEARCH_MCP_COMMAND"));
        assert!(err.contains("SEARCH_MCP_TIMEOUT_SEC must be greater than 0"));
    }
}
