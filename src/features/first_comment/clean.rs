use crate::config::Config;

pub fn should_generate_comment(post_text: &str, config: &Config) -> bool {
    post_text.contains(&config.post_signature_marker)
}

pub fn clean_post_for_llm(post_text: &str, config: &Config) -> String {
    let without_signature = match post_text.find(&config.post_signature_marker) {
        Some(index) => &post_text[..index],
        None => post_text,
    };

    without_signature.trim().to_string()
}
