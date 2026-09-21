use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceConfig {
    pub id: String,
    pub display_name: String,
    pub timezone: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramConfig {
    #[serde(default)]
    pub unknown_chat_policy: UnknownChatPolicy,
    #[serde(default)]
    pub owners: Vec<i64>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnknownChatPolicy {
    #[default]
    Ignore,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatConfig {
    pub id: i64,
    #[serde(default)]
    pub ingest: bool,
    #[serde(default)]
    pub moderation: bool,
    #[serde(default)]
    pub stats: bool,
    #[serde(default)]
    pub voice: bool,
    #[serde(default)]
    pub ask: bool,
    #[serde(default)]
    pub review_destination: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModerationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub risk_profile: String,
    #[serde(default)]
    pub review_chat: Option<String>,
    #[serde(default)]
    pub reviewer_user_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpamReputationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub sqlite_path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub private_enabled: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub default_chat: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstCommentConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub routes: Vec<FirstCommentRoute>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstCommentRoute {
    pub source_channel_id: i64,
    pub discussion_chat: String,
    #[serde(default)]
    pub post_signature_marker: String,
    #[serde(default)]
    pub invite_label: String,
    #[serde(default)]
    pub invite_url_env: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub prompt_profile: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicMcpConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskProfile {
    #[serde(default)]
    pub version: String,
    #[serde(default = "default_old_user_message_threshold")]
    pub old_user_message_threshold: i64,
    #[serde(default = "default_review_threshold")]
    pub review_threshold: i32,
    #[serde(default)]
    pub telegram_id: Option<TelegramIdRiskModel>,
}

fn default_old_user_message_threshold() -> i64 {
    5
}

fn default_review_threshold() -> i32 {
    70
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramIdRiskModel {
    pub floor: f64,
    pub ceil: f64,
    pub k: f64,
    pub midpoint_billion: f64,
    pub version: String,
}

/// Набор instance/chat policy, общий для одного процесса и одного Telegram bot token.
/// PostgreSQL и operational jobs остаются локальными для этого instance.
#[derive(Debug, Clone)]
pub struct CommunityConfig {
    pub instance: InstanceConfig,
    pub telegram: TelegramConfig,
    pub chats: BTreeMap<String, ChatConfig>,
    pub moderation: ModerationConfig,
    pub spam_reputation: SpamReputationConfig,
    pub voice: VoiceConfig,
    pub ask: AskConfig,
    pub first_comment: FirstCommentConfig,
    pub public_mcp: PublicMcpConfig,
    pub risk_profiles: BTreeMap<String, RiskProfile>,
}

/// Несекретные runtime-настройки, хранящиеся рядом с LLM profiles в TOML.
///
/// Секреты, DSN, invite/proxy URL и credentials намеренно не входят в эту
/// структуру и остаются в окружении процесса.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeSettings {
    /// Deprecated compatibility fields. Community scope belongs to the
    /// top-level instance/chat sections and these fields intentionally have no
    /// production defaults.
    #[serde(default)]
    pub source_channel_id: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)]
    pub discussion_chat_id: Option<i64>,
    pub render_timezone: String,
    #[serde(default)]
    pub chat_invite_label: Option<String>,
    #[serde(default)]
    pub post_signature_marker: Option<String>,
    pub llm_temperature: f32,
    pub llm_max_tokens: u32,
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
    pub chat_retrieval_embedding_url: String,
    pub chat_retrieval_embedding_model: String,
    pub chat_retrieval_embedding_timeout_sec: u64,
    pub chat_retrieval_embedding_query_prefix: String,
    pub chat_retrieval_embedding_document_prefix: String,
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
    pub search_mcp_tool_web: String,
    pub search_mcp_tool_github: String,
    pub search_mcp_tool_reddit: String,
    pub search_mcp_tool_fetch: Option<String>,
    pub search_fetch_top_n: usize,
    pub search_fetch_max_chars: usize,
    pub comment_blocked_source_domains: Vec<String>,
    pub comment_blocked_terms: Vec<String>,
    pub search_github_mcp_command: Option<String>,
    pub search_github_mcp_args: Vec<String>,
    pub search_github_mcp_env: Vec<String>,
    pub search_github_mcp_tools: Vec<String>,
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

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            source_channel_id: None,
            discussion_chat_id: None,
            render_timezone: "Europe/Moscow".to_string(),
            chat_invite_label: None,
            post_signature_marker: None,
            llm_temperature: 0.45,
            llm_max_tokens: 180,
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
            chat_retrieval_embedding_url: "http://127.0.0.1:8795".to_string(),
            chat_retrieval_embedding_model: "ggml-org/embeddinggemma-300M-qat-q4_0-GGUF"
                .to_string(),
            chat_retrieval_embedding_timeout_sec: 30,
            chat_retrieval_embedding_query_prefix: "task: search result | query: ".to_string(),
            chat_retrieval_embedding_document_prefix: "title: none | text: ".to_string(),
            chat_retrieval_embedding_batch_size: 16,
            chat_retrieval_embedding_poll_sec: 5,
            chat_retrieval_shadow_enabled: false,
            chat_retrieval_evidence_enabled: false,
            chat_retrieval_evidence_min_score: 2.0,
            chat_retrieval_window_days: 30,
            chat_retrieval_half_life_days: 7.0,
            search_enabled: false,
            search_extract_temperature: 0.1,
            search_extract_max_tokens: 900,
            search_mcp_command: None,
            search_mcp_args: Vec::new(),
            search_mcp_env: Vec::new(),
            search_mcp_timeout_sec: 8,
            search_query_timeout_sec: 20,
            search_mcp_tool_web: "web_search".to_string(),
            search_mcp_tool_github: "github_search".to_string(),
            search_mcp_tool_reddit: "reddit_search".to_string(),
            search_mcp_tool_fetch: Some("web_fetch_exa".to_string()),
            search_fetch_top_n: 4,
            search_fetch_max_chars: 16_000,
            comment_blocked_source_domains: Vec::new(),
            comment_blocked_terms: Vec::new(),
            search_github_mcp_command: None,
            search_github_mcp_args: Vec::new(),
            search_github_mcp_env: vec![
                "PATH".to_string(),
                "HOME".to_string(),
                "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
            ],
            search_github_mcp_tools: vec!["search_issues".to_string(), "search_code".to_string()],
            new_user_audit_enabled: false,
            new_user_audit_max_tokens: 900,
            gemini_thinking_budget: 1024,
            owner_telegram_id: None,
            send_owner_preview: true,
            ask_enabled: false,
            ask_allow_chat_admins: true,
            ask_private_user_ids: Vec::new(),
            ask_llm_temperature: 0.2,
            ask_llm_max_tokens: 1800,
            ask_max_steps: 7,
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
            voice_asr_model: "whisper-large-v3".to_string(),
            voice_asr_temperature: 0.0,
            voice_cleanup_temperature: 0.2,
            voice_cleanup_max_tokens: 1800,
            voice_render_expandable_chapters: true,
            voice_send_full_file: true,
            public_base_url: None,
            static_files_dir: "/opt/tg-ai-bot-teloxide/static".to_string(),
        }
    }
}
