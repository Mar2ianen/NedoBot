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

const DEFAULT_LLM_PROFILES_PATH: &str = "config/llm_profiles.toml.example";

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
        let llm_profiles_path = env_optional("LLM_PROFILES_PATH")
            .or_else(|| Some(DEFAULT_LLM_PROFILES_PATH.to_string()));
        let llm_profiles = llm_profiles_path
            .as_deref()
            .map(LlmProfiles::from_path)
            .transpose()?;
        let runtime = llm_profiles
            .as_ref()
            .map(|profiles| profiles.runtime.clone())
            .unwrap_or_default();

        Ok(Self {
            source_channel_id: runtime.source_channel_id,
            discussion_chat_id: runtime.discussion_chat_id,
            chat_invite_url: env_or("CHAT_INVITE_URL", "https://t.me/+RxmPtw7Bs-IxNzEy"),
            chat_invite_label: runtime.chat_invite_label,
            post_signature_marker: runtime.post_signature_marker,
            llm_profiles_path,
            llm_profiles,
            llm_temperature: runtime.llm_temperature,
            llm_max_tokens: runtime.llm_max_tokens,
            llm_proxy_url: env_optional("LLM_PROXY_URL"),
            memory_llm_temperature: runtime.memory_llm_temperature,
            memory_llm_max_tokens: runtime.memory_llm_max_tokens,
            rag_enabled: runtime.rag_enabled,
            rag_embedding_url: runtime.rag_embedding_url,
            rag_embedding_model: runtime.rag_embedding_model,
            rag_embedding_timeout_sec: runtime.rag_embedding_timeout_sec,
            rag_top_k: runtime.rag_top_k,
            rag_min_similarity: runtime.rag_min_similarity,
            rag_temporal_half_life_days: runtime.rag_temporal_half_life_days,
            chat_retrieval_embeddings_enabled: runtime.chat_retrieval_embeddings_enabled,
            chat_retrieval_embedding_batch_size: runtime.chat_retrieval_embedding_batch_size,
            chat_retrieval_embedding_poll_sec: runtime.chat_retrieval_embedding_poll_sec,
            chat_retrieval_shadow_enabled: runtime.chat_retrieval_shadow_enabled,
            chat_retrieval_evidence_enabled: runtime.chat_retrieval_evidence_enabled,
            chat_retrieval_evidence_min_score: runtime.chat_retrieval_evidence_min_score,
            chat_retrieval_window_days: runtime.chat_retrieval_window_days,
            chat_retrieval_half_life_days: runtime.chat_retrieval_half_life_days,
            search_enabled: runtime.search_enabled,
            search_extract_temperature: runtime.search_extract_temperature,
            search_extract_max_tokens: runtime.search_extract_max_tokens,
            search_mcp_command: runtime.search_mcp_command,
            search_mcp_args: runtime.search_mcp_args,
            search_mcp_env: runtime.search_mcp_env,
            search_mcp_timeout_sec: runtime.search_mcp_timeout_sec,
            search_query_timeout_sec: runtime.search_query_timeout_sec,
            search_mcp_tools: SearchMcpTools {
                web: runtime.search_mcp_tool_web,
                github: runtime.search_mcp_tool_github,
                reddit: runtime.search_mcp_tool_reddit,
            },
            search_mcp_fetch_tool: runtime.search_mcp_tool_fetch,
            search_fetch_top_n: runtime.search_fetch_top_n,
            search_fetch_max_chars: runtime.search_fetch_max_chars,
            comment_blocked_source_domains: if runtime.comment_blocked_source_domains.is_empty() {
                DEFAULT_COMMENT_BLOCKED_SOURCE_DOMAINS
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            } else {
                runtime.comment_blocked_source_domains
            },
            comment_blocked_terms: runtime.comment_blocked_terms,
            search_github_mcp_command: runtime.search_github_mcp_command,
            search_github_mcp_args: runtime.search_github_mcp_args,
            search_github_mcp_env: runtime.search_github_mcp_env,
            search_github_mcp_tools: runtime.search_github_mcp_tools,
            groq_api_key: env_or("GROQ_API_KEY", ""),
            new_user_audit_enabled: runtime.new_user_audit_enabled,
            new_user_audit_max_tokens: runtime.new_user_audit_max_tokens,
            gemini_thinking_budget: runtime.gemini_thinking_budget,
            owner_telegram_id: runtime.owner_telegram_id,
            send_owner_preview: runtime.send_owner_preview,
            ask_enabled: runtime.ask_enabled,
            ask_allow_chat_admins: runtime.ask_allow_chat_admins,
            ask_private_user_ids: runtime.ask_private_user_ids,
            ask_llm_temperature: runtime.ask_llm_temperature,
            ask_llm_max_tokens: runtime.ask_llm_max_tokens,
            ask_max_steps: runtime.ask_max_steps,
            ask_action_timeout_sec: runtime.ask_action_timeout_sec,
            ask_total_timeout_sec: runtime.ask_total_timeout_sec,
            ask_max_concurrency: runtime.ask_max_concurrency,
            ask_db_mcp_command: runtime.ask_db_mcp_command,
            ask_db_mcp_args: runtime.ask_db_mcp_args,
            ask_db_mcp_env: runtime.ask_db_mcp_env,
            ask_db_mcp_timeout_sec: runtime.ask_db_mcp_timeout_sec,
            profile_refresh_concurrency: runtime.profile_refresh_concurrency,
            comment_custom_emoji_id: runtime.comment_custom_emoji_id,
            first_comment_max_image_mb: runtime.first_comment_max_image_mb,
            tech_custom_emoji_id: runtime.tech_custom_emoji_id,
            amd_custom_emoji_id: runtime.amd_custom_emoji_id,
            radeon_custom_emoji_id: runtime.radeon_custom_emoji_id,
            ryzen_custom_emoji_id: runtime.ryzen_custom_emoji_id,
            voice_transcription_enabled: runtime.voice_transcription_enabled,
            voice_auto_transcribe: runtime.voice_auto_transcribe,
            voice_max_duration_sec: runtime.voice_max_duration_sec,
            voice_max_file_mb: runtime.voice_max_file_mb,
            voice_short_text_max_chars: runtime.voice_short_text_max_chars,
            voice_language: runtime.voice_language,
            voice_asr_provider: runtime.voice_asr_provider,
            voice_asr_model: runtime.voice_asr_model,
            voice_asr_temperature: runtime.voice_asr_temperature,
            voice_cleanup_temperature: runtime.voice_cleanup_temperature,
            voice_cleanup_max_tokens: runtime.voice_cleanup_max_tokens,
            voice_render_expandable_chapters: runtime.voice_render_expandable_chapters,
            voice_send_full_file: runtime.voice_send_full_file,
            public_base_url: runtime.public_base_url,
            static_files_dir: runtime.static_files_dir,
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

#[cfg(test)]
fn parse_bool(key: &str, value: &str) -> anyhow::Result<bool> {
    match value {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => anyhow::bail!("{key} must be true, false, 1, or 0"),
    }
}

#[cfg(test)]
fn env_parse<T>(key: &str, default: T, expected: &str) -> anyhow::Result<T>
where
    T: std::str::FromStr,
{
    let Some(value) = env_value(key) else {
        return Ok(default);
    };
    parse_env_value(key, &value, expected)
}

#[cfg(test)]
fn env_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
}

#[cfg(test)]
fn parse_env_value<T>(key: &str, value: &str, expected: &str) -> anyhow::Result<T>
where
    T: std::str::FromStr,
{
    value
        .parse()
        .map_err(|_| anyhow::anyhow!("{key} must be {expected}"))
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
        assert_eq!(config.source_channel_id, -1001575496091);
        assert_eq!(config.discussion_chat_id, -1001932061163);
        assert_eq!(config.chat_invite_label, "чате");
        assert_eq!(config.llm_temperature, 0.35);
        assert_eq!(config.llm_max_tokens, 140);
        assert_eq!(config.owner_telegram_id, Some(5939287960));
    }

    #[test]
    fn runtime_settings_defaults_to_disabled_audit_and_reject_invalid_values() {
        let config = Config::from_env().expect("configuration must parse without audit flag");
        assert!(!config.new_user_audit_enabled);
        assert_eq!(config.new_user_audit_max_tokens, 900);

        let error = toml::from_str::<crate::config_file::RuntimeSettings>(
            "new_user_audit_enabled = 'enabled'",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("invalid type"));

        let error = toml::from_str::<crate::config_file::RuntimeSettings>(
            "new_user_audit_max_tokens = 'many'",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("invalid type"));
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
