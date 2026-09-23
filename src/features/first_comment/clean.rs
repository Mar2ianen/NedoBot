use crate::config::Config;

pub fn should_generate_comment(post_text: &str, config: &Config) -> bool {
    should_generate_comment_with_marker(post_text, &config.post_signature_marker)
}

pub fn clean_post_for_llm(post_text: &str, config: &Config) -> String {
    clean_post_for_llm_with_marker(post_text, &config.post_signature_marker)
}

pub fn should_generate_comment_with_marker(post_text: &str, marker: &str) -> bool {
    !marker.is_empty() && post_text.contains(marker)
}

pub fn clean_post_for_llm_with_marker(post_text: &str, marker: &str) -> String {
    let without_signature = match post_text.find(marker) {
        Some(index) => &post_text[..index],
        None => post_text,
    };

    without_signature.trim().to_string()
}
