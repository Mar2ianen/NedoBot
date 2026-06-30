use crate::config::Config;
use crate::telegram::render::escape_html;
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
        Some(custom_emoji_id) => format!(
            r#"<tg-emoji emoji-id="{}">😎</tg-emoji> {}"#,
            escape_html(custom_emoji_id),
            body
        ),
        None => body,
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

fn render_chat_link_placeholder(text: &str, config: &Config) -> String {
    let link = format!(
        r#"<a href="{}">{}</a>"#,
        escape_html(&config.chat_invite_url),
        escape_html(&config.chat_invite_label),
    );

    if text.contains("{CHAT_LINK}") {
        escape_html(text).replace("{CHAT_LINK}", &link)
    } else {
        format!(
            r#"{} <a href="{}">в чате</a>"#,
            escape_html(text),
            escape_html(&config.chat_invite_url)
        )
    }
}
