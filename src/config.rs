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
    pub memory_llm_temperature: f32,
    pub memory_llm_max_tokens: u32,
    pub groq_api_key: String,
    pub cerebras_api_key: String,
    pub openrouter_api_key: String,
    pub ollama_base_url: String,
    pub ollama_api_key: String,
    pub openai_compat_base_url: String,
    pub openai_compat_api_key: String,
    pub openai_compat_model: Option<String>,
    pub vision_model: String,
    pub owner_telegram_id: Option<i64>,
    pub send_owner_preview: bool,
    pub comment_custom_emoji_id: Option<String>,
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
            memory_llm_temperature: env_f32("MEMORY_LLM_TEMPERATURE", 0.2),
            memory_llm_max_tokens: env_u32("MEMORY_LLM_MAX_TOKENS", 220),
            groq_api_key: env_or("GROQ_API_KEY", ""),
            cerebras_api_key: env_or("CEREBRAS_API_KEY", ""),
            openrouter_api_key: env_or("OPENROUTER_API_KEY", ""),
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
            comment_custom_emoji_id: env_optional("COMMENT_CUSTOM_EMOJI_ID"),
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
            voice_asr_model: env_or("VOICE_ASR_MODEL", "whisper-large-v3-turbo"),
            voice_asr_temperature: env_f32("VOICE_ASR_TEMPERATURE", 0.0),
            voice_cleanup_provider: env_optional("VOICE_CLEANUP_PROVIDER"),
            voice_cleanup_model: env_optional("VOICE_CLEANUP_MODEL"),
            voice_cleanup_temperature: env_f32("VOICE_CLEANUP_TEMPERATURE", 0.2),
            voice_cleanup_max_tokens: env_u32("VOICE_CLEANUP_MAX_TOKENS", 1800),
            voice_render_expandable_chapters: env_bool("VOICE_RENDER_EXPANDABLE_CHAPTERS", true),
            voice_send_full_file: env_bool("VOICE_SEND_FULL_FILE", true),
        }
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
