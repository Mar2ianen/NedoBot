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

    let is_tech = lower.contains("amd")
        || lower.contains("windows")
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
    if text.contains("{CHAT_LINK}") {
        let mut html = Html::empty();
        for (index, part) in text.split("{CHAT_LINK}").enumerate() {
            if index > 0 {
                html.push(link(&config.chat_invite_label, &config.chat_invite_url));
            }
            html.push(Html::text(part));
        }
        html
    } else {
        let mut html = Html::text(text);
        html.push(Html::raw_trusted(" "));
        html.push(link("в чате", &config.chat_invite_url));
        html
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            memory_llm_temperature: 0.2,
            memory_llm_max_tokens: 220,
            groq_api_key: String::new(),
            openrouter_api_key: String::new(),
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
        }
    }

    #[test]
    fn replaces_chat_placeholder_with_link() {
        let html = build_comment_html("Пишите в {CHAT_LINK}", &config());

        assert!(html.contains(r#"<a href="https://t.me/+test">чате</a>"#));
        assert!(!html.contains("{CHAT_LINK}"));
    }

    #[test]
    fn adds_fallback_link_without_placeholder() {
        let html = build_comment_html("Пишите версии драйвера", &config());

        assert_eq!(
            html,
            r#"Пишите версии драйвера <a href="https://t.me/+test">в чате</a>"#
        );
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
