#[derive(Clone)]
#[allow(dead_code)]
pub struct SearchMcpTools {
    pub web: String,
    pub github: String,
    pub reddit: String,
}

const DEFAULT_COMMENT_BLOCKED_SOURCE_DOMAINS: &[&str] = &[
    "meduza.io",
    "theins.ru",
    "tvrain.tv",
    "novayagazeta.eu",
    "zona.media",
    "istories.media",
    "holod.media",
    "verstka.media",
    "proekt.media",
    "thebell.io",
    "currenttime.tv",
    "svoboda.org",
    "severreal.org",
    "ridl.io",
    "doxa.team",
    "7x7-journal.ru",
    "paperpaper.ru",
];

use crate::llm::profiles::{Egress, LlmProfiles, RouteRequirements};

#[derive(Clone)]
#[allow(dead_code)]
pub struct Config {
    pub source_channel_id: i64,
    pub discussion_chat_id: i64,
    pub chat_invite_url: String,
    pub chat_invite_label: String,
    pub post_signature_marker: String,
    pub llm_profiles_path: Option<String>,
    pub llm_profiles: Option<LlmProfiles>,
    pub llm_temperature: f32,
    pub llm_max_tokens: u32,
    pub llm_proxy_url: Option<String>,
    pub memory_llm_temperature: f32,
    pub memory_llm_max_tokens: u32,
    pub rag_enabled: bool,
    pub rag_embedding_url: String,
    pub rag_embedding_model: String,
    pub rag_embedding_timeout_sec: u64,
    pub rag_top_k: usize,
    pub rag_min_similarity: f32,
    pub rag_temporal_half_life_days: f32,
    pub chat_retrieval_embeddings_enabled: bool,
    pub chat_retrieval_embedding_batch_size: usize,
    pub chat_retrieval_embedding_poll_sec: u64,
    pub chat_retrieval_shadow_enabled: bool,
    pub chat_retrieval_evidence_enabled: bool,
    pub chat_retrieval_evidence_min_score: f64,
    pub chat_retrieval_window_days: i64,
    pub chat_retrieval_half_life_days: f64,
    pub search_enabled: bool,
    pub search_extract_temperature: f32,
    pub search_extract_max_tokens: u32,
    pub search_mcp_command: Option<String>,
    pub search_mcp_args: Vec<String>,
    pub search_mcp_env: Vec<String>,
    pub search_mcp_timeout_sec: u64,
    pub search_query_timeout_sec: u64,
    pub search_mcp_tools: SearchMcpTools,
    pub search_mcp_fetch_tool: Option<String>,
    pub search_fetch_top_n: usize,
    pub search_fetch_max_chars: usize,
    pub comment_blocked_source_domains: Vec<String>,
    pub comment_blocked_terms: Vec<String>,
    pub search_github_mcp_command: Option<String>,
    pub search_github_mcp_args: Vec<String>,
    pub search_github_mcp_env: Vec<String>,
    pub search_github_mcp_tools: Vec<String>,
    pub groq_api_key: String,
    pub new_user_audit_enabled: bool,
    pub new_user_audit_max_tokens: u32,
    pub gemini_thinking_budget: u32,
    pub owner_telegram_id: Option<i64>,
    pub send_owner_preview: bool,
    pub ask_enabled: bool,
    pub ask_allow_chat_admins: bool,
    pub ask_private_user_ids: Vec<i64>,
    pub ask_llm_temperature: f32,
    pub ask_llm_max_tokens: u32,
    pub ask_max_steps: usize,
    pub ask_action_timeout_sec: u64,
    pub ask_total_timeout_sec: u64,
    pub ask_max_concurrency: usize,
    pub ask_db_mcp_command: Option<String>,
    pub ask_db_mcp_args: Vec<String>,
    pub ask_db_mcp_env: Vec<String>,
    pub ask_db_mcp_timeout_sec: u64,
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
    pub voice_cleanup_temperature: f32,
    pub voice_cleanup_max_tokens: u32,
    pub voice_render_expandable_chapters: bool,
    pub voice_send_full_file: bool,
    pub public_base_url: Option<String>,
    pub static_files_dir: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let llm_profiles_path = env_optional("LLM_PROFILES_PATH");
        let llm_profiles = llm_profiles_path
            .as_deref()
            .map(LlmProfiles::from_path)
            .transpose()?;

        Ok(Self {
            source_channel_id: env_i64("SOURCE_CHANNEL_ID", -1001575496091)?,
            discussion_chat_id: env_i64("DISCUSSION_CHAT_ID", -1001932061163)?,
            chat_invite_url: env_or("CHAT_INVITE_URL", "https://t.me/+RxmPtw7Bs-IxNzEy"),
            chat_invite_label: env_or("CHAT_INVITE_LABEL", "Присоединяйтесь к чату"),
            post_signature_marker: env_or("POST_SIGNATURE_MARKER", "Не теряем связь"),
            llm_profiles_path,
            llm_profiles,
            llm_temperature: env_f32("LLM_TEMPERATURE", 0.45)?,
            llm_max_tokens: env_u32("LLM_MAX_TOKENS", 180)?,
            llm_proxy_url: env_optional("LLM_PROXY_URL"),
            memory_llm_temperature: env_f32("MEMORY_LLM_TEMPERATURE", 0.2)?,
            memory_llm_max_tokens: env_u32("MEMORY_LLM_MAX_TOKENS", 220)?,
            rag_enabled: env_bool("RAG_ENABLED", false)?,
            rag_embedding_url: env_or("RAG_EMBEDDING_URL", "http://127.0.0.1:8788"),
            rag_embedding_model: env_or("RAG_EMBEDDING_MODEL", "cointegrated/rubert-tiny2"),
            rag_embedding_timeout_sec: env_u64("RAG_EMBEDDING_TIMEOUT_SEC", 10)?,
            rag_top_k: env_usize("RAG_TOP_K", 6)?,
            rag_min_similarity: env_f32("RAG_MIN_SIMILARITY", 0.55)?,
            rag_temporal_half_life_days: env_f32("RAG_TEMPORAL_HALF_LIFE_DAYS", 180.0)?,
            chat_retrieval_embeddings_enabled: env_bool(
                "CHAT_RETRIEVAL_EMBEDDINGS_ENABLED",
                false,
            )?,
            chat_retrieval_embedding_batch_size: env_usize(
                "CHAT_RETRIEVAL_EMBEDDING_BATCH_SIZE",
                16,
            )?,
            chat_retrieval_embedding_poll_sec: env_u64("CHAT_RETRIEVAL_EMBEDDING_POLL_SEC", 5)?,
            chat_retrieval_shadow_enabled: env_bool("CHAT_RETRIEVAL_SHADOW_ENABLED", false)?,
            chat_retrieval_evidence_enabled: env_bool("CHAT_RETRIEVAL_EVIDENCE_ENABLED", false)?,
            chat_retrieval_evidence_min_score: env_f64("CHAT_RETRIEVAL_EVIDENCE_MIN_SCORE", 2.0)?,
            chat_retrieval_window_days: env_i64("CHAT_RETRIEVAL_WINDOW_DAYS", 30)?,
            chat_retrieval_half_life_days: env_f64("CHAT_RETRIEVAL_HALF_LIFE_DAYS", 7.0)?,
            search_enabled: env_bool("SEARCH_ENABLED", false)?,
            search_extract_temperature: env_f32("SEARCH_EXTRACT_TEMPERATURE", 0.1)?,
            search_extract_max_tokens: env_u32("SEARCH_EXTRACT_MAX_TOKENS", 900)?,
            search_mcp_command: env_optional("SEARCH_MCP_COMMAND"),
            search_mcp_args: env_args("SEARCH_MCP_ARGS"),
            search_mcp_env: env_list_csv("SEARCH_MCP_ENV"),
            search_mcp_timeout_sec: env_u64("SEARCH_MCP_TIMEOUT_SEC", 8)?,
            search_query_timeout_sec: env_u64("SEARCH_QUERY_TIMEOUT_SEC", 20)?,
            search_mcp_tools: SearchMcpTools {
                web: env_or("SEARCH_MCP_TOOL_WEB", "web_search"),
                github: env_or("SEARCH_MCP_TOOL_GITHUB", "github_search"),
                reddit: env_or("SEARCH_MCP_TOOL_REDDIT", "reddit_search"),
            },
            search_mcp_fetch_tool: env_optional("SEARCH_MCP_TOOL_FETCH")
                .or_else(|| Some("web_fetch_exa".to_string())),
            search_fetch_top_n: env_usize("SEARCH_FETCH_TOP_N", 4)?,
            search_fetch_max_chars: env_usize("SEARCH_FETCH_MAX_CHARS", 16_000)?,
            comment_blocked_source_domains: env_list_csv_or(
                "COMMENT_BLOCKED_SOURCE_DOMAINS",
                DEFAULT_COMMENT_BLOCKED_SOURCE_DOMAINS,
            ),
            comment_blocked_terms: env_list_csv("COMMENT_BLOCKED_TERMS"),
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
            new_user_audit_enabled: env_bool("NEW_USER_AUDIT_ENABLED", false)?,
            new_user_audit_max_tokens: env_u32("NEW_USER_AUDIT_MAX_TOKENS", 900)?,
            gemini_thinking_budget: env_u32("GEMINI_THINKING_BUDGET", 1024)?,
            owner_telegram_id: env_optional_i64("OWNER_TELEGRAM_ID")?,
            send_owner_preview: env_bool("SEND_OWNER_PREVIEW", true)?,
            ask_enabled: env_bool("ASK_ENABLED", false)?,
            ask_allow_chat_admins: env_bool("ASK_ALLOW_CHAT_ADMINS", true)?,
            ask_private_user_ids: env_i64_list_csv("ASK_PRIVATE_USER_IDS")?,
            ask_llm_temperature: env_f32("ASK_LLM_TEMPERATURE", 0.2)?,
            ask_llm_max_tokens: env_u32("ASK_LLM_MAX_TOKENS", 1800)?,
            ask_max_steps: env_usize("ASK_MAX_STEPS", 7)?,
            ask_action_timeout_sec: env_u64("ASK_ACTION_TIMEOUT_SEC", 45)?,
            ask_total_timeout_sec: env_u64("ASK_TOTAL_TIMEOUT_SEC", 180)?,
            ask_max_concurrency: env_usize("ASK_MAX_CONCURRENCY", 1)?,
            ask_db_mcp_command: env_optional("ASK_DB_MCP_COMMAND"),
            ask_db_mcp_args: env_args("ASK_DB_MCP_ARGS"),
            ask_db_mcp_env: env_list_csv_or(
                "ASK_DB_MCP_ENV",
                &["ASK_DATABASE_URL", "MCP_MANIFEST"],
            ),
            ask_db_mcp_timeout_sec: env_u64("ASK_DB_MCP_TIMEOUT_SEC", 8)?,
            profile_refresh_concurrency: env_usize("PROFILE_REFRESH_CONCURRENCY", 4)?,
            comment_custom_emoji_id: env_optional("COMMENT_CUSTOM_EMOJI_ID"),
            first_comment_max_image_mb: env_u32("FIRST_COMMENT_MAX_IMAGE_MB", 10)?,
            tech_custom_emoji_id: env_optional("TECH_CUSTOM_EMOJI_ID"),
            amd_custom_emoji_id: env_optional("AMD_CUSTOM_EMOJI_ID"),
            radeon_custom_emoji_id: env_optional("RADEON_CUSTOM_EMOJI_ID"),
            ryzen_custom_emoji_id: env_optional("RYZEN_CUSTOM_EMOJI_ID"),
            voice_transcription_enabled: env_bool("VOICE_TRANSCRIPTION_ENABLED", false)?,
            voice_auto_transcribe: env_bool("VOICE_AUTO_TRANSCRIBE", false)?,
            voice_max_duration_sec: env_u32("VOICE_MAX_DURATION_SEC", 600)?,
            voice_max_file_mb: env_u32("VOICE_MAX_FILE_MB", 20)?,
            voice_short_text_max_chars: env_usize("VOICE_SHORT_TEXT_MAX_CHARS", 400)?,
            voice_language: env_or("VOICE_LANGUAGE", "ru"),
            voice_asr_provider: env_or("VOICE_ASR_PROVIDER", "groq"),
            voice_asr_model: env_or("VOICE_ASR_MODEL", "whisper-large-v3"),
            voice_asr_temperature: env_f32("VOICE_ASR_TEMPERATURE", 0.0)?,
            voice_cleanup_temperature: env_f32("VOICE_CLEANUP_TEMPERATURE", 0.2)?,
            voice_cleanup_max_tokens: env_u32("VOICE_CLEANUP_MAX_TOKENS", 1800)?,
            voice_render_expandable_chapters: env_bool("VOICE_RENDER_EXPANDABLE_CHAPTERS", true)?,
            voice_send_full_file: env_bool("VOICE_SEND_FULL_FILE", true)?,
            public_base_url: env_optional("PUBLIC_BASE_URL"),
            static_files_dir: env_or("STATIC_FILES_DIR", "/opt/tg-ai-bot-teloxide/static"),
        })
    }

    pub fn validate_runtime_secrets(&self) -> anyhow::Result<()> {
        let mut errors = Vec::new();

        if self.llm_profiles.is_some() {
            self.validate_profile_routes(&mut errors);
        } else {
            errors.push("LLM_PROFILES_PATH must configure authoritative LLM routes".to_string());
        }

        if self.search_enabled {
            validate_search_config(&mut errors, self);
        }

        if self.voice_transcription_enabled {
            validate_voice_asr_secret(&mut errors, self);
        }

        if self.rag_enabled {
            self.validate_rag_retrieval_config(&mut errors);
        }
        self.validate_chat_retrieval_config(&mut errors);

        if self.new_user_audit_enabled {
            require_positive(
                &mut errors,
                "NEW_USER_AUDIT_MAX_TOKENS",
                self.new_user_audit_max_tokens,
            );
            self.validate_embedding_config(&mut errors);
        }
        if self.profile_refresh_concurrency == 0 {
            errors.push("PROFILE_REFRESH_CONCURRENCY must be greater than 0".to_string());
        }

        if self.ask_enabled {
            if self.owner_telegram_id.is_none() {
                errors.push("ASK_ENABLED=true requires OWNER_TELEGRAM_ID".to_string());
            }
            if self.ask_max_steps == 0 {
                errors.push("ASK_MAX_STEPS must be greater than 0".to_string());
            }
            if self.ask_action_timeout_sec == 0 {
                errors.push("ASK_ACTION_TIMEOUT_SEC must be greater than 0".to_string());
            }
            if self.ask_total_timeout_sec == 0 {
                errors.push("ASK_TOTAL_TIMEOUT_SEC must be greater than 0".to_string());
            }
            if self.ask_max_concurrency == 0 {
                errors.push("ASK_MAX_CONCURRENCY must be greater than 0".to_string());
            }
            if self.ask_db_mcp_command.is_none() {
                errors.push("ASK_ENABLED=true requires ASK_DB_MCP_COMMAND".to_string());
            }
            if self.ask_db_mcp_timeout_sec == 0 {
                errors.push("ASK_DB_MCP_TIMEOUT_SEC must be greater than 0".to_string());
            }
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

    fn validate_profile_routes(&self, errors: &mut Vec<String>) {
        let Some(profiles) = self.llm_profiles.as_ref() else {
            return;
        };
        let mut routes = vec![
            (
                "first_comment",
                RouteRequirements {
                    requires_system_prompt: true,
                    num_predict: Some(self.llm_max_tokens),
                    ..RouteRequirements::default()
                },
            ),
            (
                "first_comment",
                RouteRequirements {
                    requires_images: true,
                    requires_system_prompt: true,
                    num_predict: Some(self.llm_max_tokens),
                    ..RouteRequirements::default()
                },
            ),
        ];
        if self.rag_enabled {
            routes.push((
                "memory",
                RouteRequirements {
                    requires_system_prompt: true,
                    num_predict: Some(self.memory_llm_max_tokens),
                    ..RouteRequirements::default()
                },
            ));
        }
        if self.voice_transcription_enabled {
            routes.push((
                "voice_cleanup",
                RouteRequirements {
                    requires_system_prompt: true,
                    num_predict: Some(self.voice_cleanup_max_tokens),
                    ..RouteRequirements::default()
                },
            ));
        }
        if self.search_enabled {
            routes.push((
                "search_extract",
                RouteRequirements {
                    requires_system_prompt: true,
                    num_predict: Some(self.search_extract_max_tokens),
                    ..RouteRequirements::default()
                },
            ));
        }
        if self.new_user_audit_enabled {
            for requires_images in [false, true] {
                routes.push((
                    "new_user_audit",
                    RouteRequirements {
                        requires_images,
                        requires_system_prompt: true,
                        // Runtime допускает json_schema, json_object и prompt_only:
                        // окончательный контракт контролирует Rust validator.
                        num_predict: Some(self.new_user_audit_max_tokens),
                        ..RouteRequirements::default()
                    },
                ));
            }
        }
        if self.ask_enabled {
            for requires_images in [false, true] {
                routes.push((
                    "ask",
                    RouteRequirements {
                        requires_images,
                        requires_tools: true,
                        requires_system_prompt: true,
                        num_predict: Some(self.ask_llm_max_tokens),
                        ..RouteRequirements::default()
                    },
                ));
            }
        }

        let mut required_secrets = std::collections::BTreeMap::new();
        let mut required_proxy_providers = std::collections::BTreeMap::new();
        for (route, requirements) in routes {
            match profiles.resolve_route(route, &requirements) {
                Ok(resolved) => {
                    for selection in resolved.selections {
                        required_secrets
                            .entry(selection.provider.api_key_env.as_str())
                            .or_insert((selection.provider_key, route));
                        if selection.provider.egress == Egress::Proxy {
                            required_proxy_providers
                                .entry(selection.provider_key)
                                .or_insert(route);
                        }
                    }
                }
                Err(error) => {
                    errors.push(format!("LLM profile route {route:?} is invalid: {error}"))
                }
            }
        }
        for (api_key_env, (provider_key, route)) in required_secrets {
            let configured = std::env::var(api_key_env).is_ok_and(|value| !value.trim().is_empty());
            if !configured {
                errors.push(format!(
                    "LLM profile route {route:?} requires non-empty {api_key_env} for provider {provider_key:?}"
                ));
            }
        }
        if self
            .llm_proxy_url
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            for (provider_key, route) in required_proxy_providers {
                errors.push(format!(
                    "LLM profile route {route:?} selects proxy egress for provider {provider_key:?}, but LLM_PROXY_URL is not configured"
                ));
            }
        }
    }

    fn validate_rag_retrieval_config(&self, errors: &mut Vec<String>) {
        self.validate_embedding_config(errors);
        require_positive(errors, "RAG_TOP_K", self.rag_top_k);
        require_in_unit_interval(errors, "RAG_MIN_SIMILARITY", self.rag_min_similarity);
        require_positive(
            errors,
            "RAG_TEMPORAL_HALF_LIFE_DAYS",
            self.rag_temporal_half_life_days,
        );
    }

    fn validate_chat_retrieval_config(&self, errors: &mut Vec<String>) {
        if self.chat_retrieval_evidence_enabled && !self.chat_retrieval_shadow_enabled {
            errors.push(
                "CHAT_RETRIEVAL_EVIDENCE_ENABLED=true requires CHAT_RETRIEVAL_SHADOW_ENABLED=true"
                    .to_string(),
            );
        }
        if self.chat_retrieval_shadow_enabled && !self.chat_retrieval_embeddings_enabled {
            errors.push(
                "CHAT_RETRIEVAL_SHADOW_ENABLED=true requires CHAT_RETRIEVAL_EMBEDDINGS_ENABLED=true"
                    .to_string(),
            );
        }
        if !self.chat_retrieval_embeddings_enabled {
            return;
        }

        self.validate_embedding_config(errors);
        require_positive(
            errors,
            "CHAT_RETRIEVAL_EMBEDDING_BATCH_SIZE",
            self.chat_retrieval_embedding_batch_size,
        );
        require_positive(
            errors,
            "CHAT_RETRIEVAL_EMBEDDING_POLL_SEC",
            self.chat_retrieval_embedding_poll_sec,
        );
        require_positive(
            errors,
            "CHAT_RETRIEVAL_WINDOW_DAYS",
            self.chat_retrieval_window_days,
        );
        require_positive(
            errors,
            "CHAT_RETRIEVAL_HALF_LIFE_DAYS",
            self.chat_retrieval_half_life_days,
        );
    }

    fn validate_embedding_config(&self, errors: &mut Vec<String>) {
        require_http_url(errors, "RAG_EMBEDDING_URL", &self.rag_embedding_url);
        require_non_empty(errors, "RAG_EMBEDDING_MODEL", &self.rag_embedding_model);
        require_positive(
            errors,
            "RAG_EMBEDDING_TIMEOUT_SEC",
            self.rag_embedding_timeout_sec,
        );
    }
}

fn require_non_empty(errors: &mut Vec<String>, key: &str, value: &str) {
    if value.trim().is_empty() {
        errors.push(format!("{key} must not be empty"));
    }
}

fn require_http_url(errors: &mut Vec<String>, key: &str, value: &str) {
    if value.trim().is_empty() {
        errors.push(format!("{key} must not be empty"));
        return;
    }
    let valid = reqwest::Url::parse(value)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some());
    if !valid {
        errors.push(format!("{key} must be an absolute HTTP(S) URL"));
    }
}

fn require_positive<T>(errors: &mut Vec<String>, key: &str, value: T)
where
    T: PartialOrd + From<u8>,
{
    if value <= T::from(0) {
        errors.push(format!("{key} must be greater than 0"));
    }
}

fn require_in_unit_interval(errors: &mut Vec<String>, key: &str, value: f32) {
    if !(0.0..=1.0).contains(&value) {
        errors.push(format!("{key} must be between 0 and 1"));
    }
}

fn validate_search_config(errors: &mut Vec<String>, config: &Config) {
    if config.search_mcp_command.is_none() {
        errors.push("SEARCH_ENABLED=true requires non-empty SEARCH_MCP_COMMAND".to_string());
    }

    if config.search_mcp_timeout_sec == 0 {
        errors.push("SEARCH_MCP_TIMEOUT_SEC must be greater than 0".to_string());
    }

    if config.search_query_timeout_sec == 0 {
        errors.push("SEARCH_QUERY_TIMEOUT_SEC must be greater than 0".to_string());
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

fn require_secret(errors: &mut Vec<String>, key: &str, value: &str, context: &str) {
    if value.trim().is_empty() {
        errors.push(format!("{context} requires non-empty {key}"));
    }
}

fn env_bool(key: &str, default: bool) -> anyhow::Result<bool> {
    Ok(env_value(key)
        .map(|value| parse_bool(key, &value))
        .transpose()?
        .unwrap_or(default))
}

fn parse_bool(key: &str, value: &str) -> anyhow::Result<bool> {
    match value {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => anyhow::bail!("{key} must be true, false, 1, or 0"),
    }
}

fn env_i64(key: &str, default: i64) -> anyhow::Result<i64> {
    env_parse(key, default, "a signed 64-bit integer")
}

fn env_u32(key: &str, default: u32) -> anyhow::Result<u32> {
    env_parse(key, default, "a non-negative 32-bit integer")
}

fn env_u64(key: &str, default: u64) -> anyhow::Result<u64> {
    env_parse(key, default, "a non-negative 64-bit integer")
}

fn env_usize(key: &str, default: usize) -> anyhow::Result<usize> {
    env_parse(key, default, "a non-negative integer")
}

fn env_f32(key: &str, default: f32) -> anyhow::Result<f32> {
    env_parse(key, default, "a number")
}

fn env_f64(key: &str, default: f64) -> anyhow::Result<f64> {
    env_parse(key, default, "a number")
}

fn env_parse<T>(key: &str, default: T, expected: &str) -> anyhow::Result<T>
where
    T: std::str::FromStr,
{
    let Some(value) = env_value(key) else {
        return Ok(default);
    };
    parse_env_value(key, &value, expected)
}

fn parse_env_value<T>(key: &str, value: &str, expected: &str) -> anyhow::Result<T>
where
    T: std::str::FromStr,
{
    value
        .parse()
        .map_err(|_| anyhow::anyhow!("{key} must be {expected}"))
}

fn env_optional_i64(key: &str) -> anyhow::Result<Option<i64>> {
    let Some(value) = env_optional(key) else {
        return Ok(None);
    };
    value
        .parse()
        .map(Some)
        .map_err(|_| anyhow::anyhow!("{key} must be a signed 64-bit integer"))
}

fn env_i64_list_csv(key: &str) -> anyhow::Result<Vec<i64>> {
    env_list_csv(key)
        .into_iter()
        .map(|value| {
            value
                .parse()
                .map_err(|_| anyhow::anyhow!("{key} must contain only signed 64-bit integers"))
        })
        .collect()
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
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
    use std::{
        ffi::OsString,
        sync::{LazyLock, Mutex},
    };

    use super::*;

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    struct EnvVarGuard {
        key: &'static str,
        original_value: Option<OsString>,
    }

    impl EnvVarGuard {
        fn unset(key: &'static str) -> Self {
            let original_value = std::env::var_os(key);
            unsafe { std::env::remove_var(key) };
            Self {
                key,
                original_value,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.original_value {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

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
            ask_db_mcp_env: vec!["ASK_DATABASE_URL".to_string(), "MCP_MANIFEST".to_string()],
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

    #[test]
    fn from_env_defaults_ask_db_mcp_env_to_database_url_and_manifest() {
        let _env_lock = ENV_LOCK
            .lock()
            .expect("environment test lock must not be poisoned");
        let _ask_db_mcp_env = EnvVarGuard::unset("ASK_DB_MCP_ENV");

        let config =
            Config::from_env().expect("configuration must parse with default MCP environment");

        assert_eq!(config.ask_db_mcp_env, ["ASK_DATABASE_URL", "MCP_MANIFEST"]);
    }

    #[test]
    fn new_user_audit_flag_defaults_to_disabled_and_rejects_invalid_values() {
        let _env_lock = ENV_LOCK
            .lock()
            .expect("environment test lock must not be poisoned");
        let _new_user_audit_enabled = EnvVarGuard::unset("NEW_USER_AUDIT_ENABLED");
        let _new_user_audit_max_tokens = EnvVarGuard::unset("NEW_USER_AUDIT_MAX_TOKENS");

        let config = Config::from_env().expect("configuration must parse without audit flag");
        assert!(!config.new_user_audit_enabled);
        assert_eq!(config.new_user_audit_max_tokens, 900);

        unsafe { std::env::set_var("NEW_USER_AUDIT_ENABLED", "enabled") };
        let error = match Config::from_env() {
            Ok(_) => panic!("invalid audit flag must fail configuration parsing"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("NEW_USER_AUDIT_ENABLED"));

        unsafe { std::env::set_var("NEW_USER_AUDIT_ENABLED", "false") };
        unsafe { std::env::set_var("NEW_USER_AUDIT_MAX_TOKENS", "many") };
        let error = match Config::from_env() {
            Ok(_) => panic!("invalid audit token limit must fail configuration parsing"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("NEW_USER_AUDIT_MAX_TOKENS"));
    }

    #[test]
    fn enabled_new_user_audit_rejects_zero_output_budget() {
        let mut config = config();
        config.new_user_audit_enabled = true;
        config.new_user_audit_max_tokens = 0;

        let error = config.validate_runtime_secrets().unwrap_err().to_string();
        assert!(error.contains("NEW_USER_AUDIT_MAX_TOKENS must be greater than 0"));
    }

    #[test]
    fn strict_bool_parser_accepts_documented_values_and_rejects_invalid_input() {
        assert!(parse_bool("VOICE_AUTO_TRANSCRIBE", "true").unwrap());
        assert!(parse_bool("VOICE_AUTO_TRANSCRIBE", "1").unwrap());
        assert!(!parse_bool("VOICE_AUTO_TRANSCRIBE", "false").unwrap());
        assert!(!parse_bool("VOICE_AUTO_TRANSCRIBE", "0").unwrap());

        let error = parse_bool("VOICE_AUTO_TRANSCRIBE", "TRUE")
            .unwrap_err()
            .to_string();
        assert!(error.contains("VOICE_AUTO_TRANSCRIBE"));
    }

    #[test]
    fn scalar_env_parser_uses_default_only_when_variable_is_absent() {
        const TEST_KEY: &str = "UNSET_CONFIG_TEST_VALUE";
        assert!(std::env::var_os(TEST_KEY).is_none());
        assert_eq!(env_parse(TEST_KEY, 42_u64, "a test number").unwrap(), 42);

        let error = parse_env_value::<u64>(
            "RAG_EMBEDDING_TIMEOUT_SEC",
            "ten",
            "a non-negative 64-bit integer",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("RAG_EMBEDDING_TIMEOUT_SEC"));
    }

    #[test]
    fn profile_mode_validates_route_fallback_secret_environment_names() {
        let _lock = ENV_LOCK.lock().unwrap();
        let primary = EnvVarGuard::unset("PROFILE_PRIMARY_TEST_KEY");
        let fallback = EnvVarGuard::unset("PROFILE_FALLBACK_TEST_KEY");
        let mut config = config();
        config.llm_profiles = Some(
            LlmProfiles::from_toml(
                r#"
[providers.primary]
driver = "openai_compatible"
base_url = "https://primary.example/v1"
api_key_env = "PROFILE_PRIMARY_TEST_KEY"
[providers.fallback]
driver = "openai_compatible"
base_url = "https://fallback.example/v1"
api_key_env = "PROFILE_FALLBACK_TEST_KEY"

[models.primary]
provider = "primary"
model = "primary-model"
[models.primary.capabilities]
supports_images = true
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
structured_output = "prompt_only"
context_window_tokens = 4096
max_output_tokens = 256
request_timeout_sec = 5
thinking = "none"

[routes.first_comment]
models = ["primary", "fallback"]
"#,
            )
            .unwrap(),
        );

        let error = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(error.contains("PROFILE_PRIMARY_TEST_KEY"));
        assert!(error.contains("PROFILE_FALLBACK_TEST_KEY"));
        drop(primary);
        drop(fallback);
    }

    #[test]
    fn profile_mode_keeps_voice_asr_secret_validation() {
        let _lock = ENV_LOCK.lock().unwrap();
        let gemini = EnvVarGuard::unset("GEMINI_API_KEY");
        let ollama = EnvVarGuard::unset("OLLAMA_API_KEY");
        let groq = EnvVarGuard::unset("GROQ_API_KEY");
        unsafe {
            std::env::set_var("GEMINI_API_KEY", "test-key");
            std::env::set_var("OLLAMA_API_KEY", "test-key");
        }
        let mut config = config();
        config.llm_profiles = Some(
            LlmProfiles::from_toml(include_str!("../config/llm_profiles.toml.example")).unwrap(),
        );
        config.voice_transcription_enabled = true;
        config.voice_auto_transcribe = true;

        let error = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(error.contains("VOICE_ASR_PROVIDER=groq requires non-empty GROQ_API_KEY"));
        drop(gemini);
        drop(ollama);
        drop(groq);
    }

    #[test]
    fn profile_mode_uses_route_profiles_for_memory() {
        let _lock = ENV_LOCK.lock().unwrap();
        let gemini = EnvVarGuard::unset("GEMINI_API_KEY");
        let ollama = EnvVarGuard::unset("OLLAMA_API_KEY");
        unsafe {
            std::env::set_var("GEMINI_API_KEY", "test-key");
            std::env::set_var("OLLAMA_API_KEY", "test-key");
        }
        let mut config = config();
        config.llm_profiles = Some(
            LlmProfiles::from_toml(include_str!("../config/llm_profiles.toml.example")).unwrap(),
        );
        config.rag_enabled = true;

        config.validate_runtime_secrets().unwrap();

        drop(gemini);
        drop(ollama);
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
    fn missing_profiles_are_rejected_before_runtime() {
        let config = config();

        let error = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(error.contains("LLM_PROFILES_PATH must configure authoritative LLM routes"));
    }

    #[test]
    fn enabled_rag_validates_retrieval_limits() {
        let mut config = config();
        config.rag_enabled = true;
        config.rag_top_k = 0;
        config.rag_min_similarity = 1.5;
        config.rag_temporal_half_life_days = 0.0;

        let error = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(error.contains("RAG_TOP_K"));
        assert!(error.contains("RAG_MIN_SIMILARITY"));
        assert!(error.contains("RAG_TEMPORAL_HALF_LIFE_DAYS"));
    }

    #[test]
    fn enabled_new_user_audit_requires_its_embedding_config() {
        let mut config = config();
        config.new_user_audit_enabled = true;
        config.rag_embedding_url.clear();

        let error = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(error.contains("RAG_EMBEDDING_URL must not be empty"));
    }

    #[test]
    fn enabled_new_user_audit_requires_image_capable_json_schema_profile_route() {
        let _lock = ENV_LOCK.lock().unwrap();
        let gemini = EnvVarGuard::unset("GEMINI_API_KEY");
        unsafe { std::env::set_var("GEMINI_API_KEY", "test-key") };

        let mut config = config();
        config.llm_profiles = Some(
            LlmProfiles::from_toml(
                r#"
[providers.test]
driver = "openai_compatible"
base_url = "https://test.example/v1"
api_key_env = "GEMINI_API_KEY"

[models.text_only]
provider = "test"
model = "test-model"
[models.text_only.capabilities]
supports_images = false
supports_tools = false
supports_system_prompt = true
structured_output = "json_schema"
context_window_tokens = 4096
max_output_tokens = 256
request_timeout_sec = 5
thinking = "none"

[routes.first_comment]
models = ["text_only"]
[routes.new_user_audit]
models = ["text_only"]
"#,
            )
            .unwrap(),
        );
        config.new_user_audit_enabled = true;

        let error = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(error.contains("LLM profile route \"new_user_audit\" is invalid"));
        assert!(error.contains("requires images"));
        drop(gemini);
    }

    #[test]
    fn new_user_audit_profile_validation_requires_prompt_only_fallback_secret() {
        let _lock = ENV_LOCK.lock().unwrap();
        let primary = EnvVarGuard::unset("TEST_AUDIT_PRIMARY_KEY");
        let fallback = EnvVarGuard::unset("TEST_AUDIT_FALLBACK_KEY");
        unsafe { std::env::set_var("TEST_AUDIT_PRIMARY_KEY", "test-key") };

        let mut config = config();
        config.llm_profiles = Some(
            LlmProfiles::from_toml(
                r#"
[providers.primary]
driver = "openai_compatible"
base_url = "https://primary.example/v1"
api_key_env = "TEST_AUDIT_PRIMARY_KEY"
[providers.fallback]
driver = "openai_compatible"
base_url = "https://fallback.example/v1"
api_key_env = "TEST_AUDIT_FALLBACK_KEY"

[models.primary]
provider = "primary"
model = "primary-model"
[models.primary.capabilities]
supports_images = true
supports_tools = false
supports_system_prompt = true
structured_output = "json_schema"
context_window_tokens = 4096
max_output_tokens = 1024
request_timeout_sec = 5
thinking = "none"

[models.fallback]
provider = "fallback"
model = "fallback-model"
[models.fallback.capabilities]
supports_images = true
supports_tools = false
supports_system_prompt = true
structured_output = "prompt_only"
context_window_tokens = 4096
max_output_tokens = 1024
request_timeout_sec = 5
thinking = "none"

[routes.first_comment]
models = ["primary"]
[routes.new_user_audit]
models = ["primary", "fallback"]
"#,
            )
            .unwrap(),
        );
        config.new_user_audit_enabled = true;

        let error = config.validate_runtime_secrets().unwrap_err().to_string();
        assert!(error.contains("TEST_AUDIT_FALLBACK_KEY"));
        drop(primary);
        drop(fallback);
    }

    #[test]
    fn shadow_retrieval_requires_embedding_ingestion_and_embedding_config() {
        let mut config = config();
        config.chat_retrieval_shadow_enabled = true;
        config.rag_embedding_url.clear();

        let error = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(error.contains(
            "CHAT_RETRIEVAL_SHADOW_ENABLED=true requires CHAT_RETRIEVAL_EMBEDDINGS_ENABLED=true"
        ));
        assert!(!error.contains("RAG_EMBEDDING_URL must not be empty"));

        config.chat_retrieval_embeddings_enabled = true;
        let error = config.validate_runtime_secrets().unwrap_err().to_string();
        assert!(error.contains("RAG_EMBEDDING_URL must not be empty"));
    }

    #[test]
    fn chat_retrieval_evidence_requires_shadow_retrieval() {
        let mut config = config();
        config.chat_retrieval_evidence_enabled = true;

        let error = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(error.contains(
            "CHAT_RETRIEVAL_EVIDENCE_ENABLED=true requires CHAT_RETRIEVAL_SHADOW_ENABLED=true"
        ));
    }

    #[test]
    fn embedding_ingestion_requires_an_absolute_http_endpoint() {
        let mut config = config();
        config.chat_retrieval_embeddings_enabled = true;
        config.rag_embedding_url = "not a URL".to_string();

        let error = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(error.contains("RAG_EMBEDDING_URL must be an absolute HTTP(S) URL"));
    }

    #[test]
    fn disabled_search_does_not_validate_mcp_command() {
        let mut config = config();
        config.search_enabled = false;
        config.search_mcp_command = None;
        config.search_mcp_timeout_sec = 0;

        let error = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(error.contains("LLM_PROFILES_PATH"));
        assert!(!error.contains("SEARCH_ENABLED=true requires"));
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

    #[test]
    fn enabled_ask_requires_owner_model_and_mcp_command() {
        let mut config = config();
        config.ask_enabled = true;
        config.ask_db_mcp_command = None;

        let err = config.validate_runtime_secrets().unwrap_err().to_string();

        assert!(err.contains("ASK_ENABLED=true requires OWNER_TELEGRAM_ID"));
        assert!(err.contains("LLM_PROFILES_PATH must configure authoritative LLM routes"));
        assert!(err.contains("ASK_ENABLED=true requires ASK_DB_MCP_COMMAND"));
    }
}
