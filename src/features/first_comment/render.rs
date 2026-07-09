use crate::config::Config;
use crate::telegram::html::{Html, link};
use crate::text::{normalize_ai_markers, strip_links};

pub fn build_comment_html(llm_body: &str, config: &Config) -> String {
    // The model decides the wording; code owns links and custom emoji markup.
    let clean_body = normalize_ai_markers(&strip_links(llm_body))
        .trim()
        .to_string();

    if clean_body.is_empty() {
        return String::new();
    }

    let body = render_chat_link_placeholder(&clean_body, config);

    match pick_comment_emoji(llm_body, config) {
        Some(custom_emoji_id) => {
            let mut html = Html::empty();
            html.push(Html::custom_emoji(custom_emoji_id, "😎"));
            html.push(Html::raw_trusted(" "));
            html.push(body);
            html.into_string()
        }
        None => body.into_string(),
    }
}

fn pick_comment_emoji<'a>(text: &str, config: &'a Config) -> Option<&'a str> {
    let lower = text.to_lowercase();

    if lower.contains("radeon") || lower.contains("видеокарт") {
        return config
            .radeon_custom_emoji_id
            .as_deref()
            .or(config.amd_custom_emoji_id.as_deref())
            .or(config.comment_custom_emoji_id.as_deref());
    }

    if lower.contains("ryzen") {
        return config
            .ryzen_custom_emoji_id
            .as_deref()
            .or(config.amd_custom_emoji_id.as_deref())
            .or(config.comment_custom_emoji_id.as_deref());
    }

    if lower.contains("amd") {
        return config
            .amd_custom_emoji_id
            .as_deref()
            .or(config.comment_custom_emoji_id.as_deref());
    }

    let is_tech = lower.contains("windows")
        || lower.contains("драйвер")
        || lower.contains("fps")
        || lower.contains("пк")
        || lower.contains("видеокарт");

    if is_tech {
        config
            .tech_custom_emoji_id
            .as_deref()
            .or(config.comment_custom_emoji_id.as_deref())
    } else {
        config.comment_custom_emoji_id.as_deref()
    }
}

fn render_chat_link_placeholder(text: &str, config: &Config) -> Html {
    let mut html = Html::empty();
    let mut rest = text;
    while let Some(start) = rest.find("{CHAT_LINK") {
        let (before, after_start) = rest.split_at(start);
        html.push(Html::text(before));

        let Some(end) = after_start.find('}') else {
            html.push(Html::text(after_start));
            return html;
        };

        let token = &after_start[..=end];
        if let Some(label) = chat_link_label(token, config) {
            html.push(link(&label, &config.chat_invite_url));
        } else {
            html.push(Html::text(token));
        }

        rest = &after_start[end + 1..];
    }

    html.push(Html::text(rest));

    html
}

fn chat_link_label(token: &str, config: &Config) -> Option<String> {
    if token == "{CHAT_LINK}" {
        return Some(config.chat_invite_label.clone());
    }

    let label = token
        .strip_prefix("{CHAT_LINK:")
        .and_then(|value| value.strip_suffix('}'))?
        .trim();

    allowed_chat_link_label(label).map(str::to_string)
}

fn allowed_chat_link_label(label: &str) -> Option<&'static str> {
    match label.to_lowercase().as_str() {
        "чат" => Some("чат"),
        "чате" => Some("чате"),
        "чатик" => Some("чатик"),
        "чатике" => Some("чатике"),
        "обсуждение" => Some("обсуждение"),
        "обсуждении" => Some("обсуждении"),
        "комменты" => Some("комменты"),
        "комментах" => Some("комментах"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SearchMcpTools;

    fn config() -> Config {
        Config {
            source_channel_id: -1001,
            discussion_chat_id: -1002,
            chat_invite_url: "https://t.me/+test".to_string(),
            chat_invite_label: "чате".to_string(),
            post_signature_marker: "Не теряем связь".to_string(),
            llm_provider: "ollama".to_string(),
            llm_model: Some("gemma4:31b".to_string()),
            llm_supports_images: Some(true),
            llm_temperature: 0.45,
            llm_max_tokens: 140,
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
    fn replaces_chat_placeholder_with_link() {
        let html = build_comment_html("Пишите в {CHAT_LINK}", &config());

        assert!(html.contains(r#"<a href="https://t.me/+test">чате</a>"#));
        assert!(!html.contains("{CHAT_LINK}"));
    }

    #[test]
    fn replaces_chat_placeholder_with_custom_safe_label() {
        let html = build_comment_html("Несите частоты в {CHAT_LINK:чатик}", &config());

        assert_eq!(
            html,
            r#"Несите частоты в <a href="https://t.me/+test">чатик</a>"#
        );
    }

    #[test]
    fn ignores_unknown_chat_placeholder_label_without_fallback() {
        let html = build_comment_html("Несите частоты в {CHAT_LINK:<b>ловушку</b>}", &config());

        assert_eq!(
            html,
            "Несите частоты в {CHAT_LINK:&lt;b&gt;ловушку&lt;/b&gt;}"
        );
    }

    #[test]
    fn does_not_add_fallback_link_without_placeholder() {
        let html = build_comment_html("Пишите версии драйвера", &config());

        assert_eq!(html, "Пишите версии драйвера");
    }

    #[test]
    fn escapes_model_html() {
        let html = build_comment_html("<b>сырой html</b> в {CHAT_LINK}", &config());

        assert!(html.contains("&lt;b&gt;сырой html&lt;/b&gt;"));
        assert!(!html.contains("<b>сырой html</b>"));
    }

    #[test]
    fn strips_model_links() {
        let html = build_comment_html("Тест https://example.com в {CHAT_LINK}", &config());

        assert!(!html.contains("https://example.com"));
        assert!(html.contains("Тест в "));
    }
}
