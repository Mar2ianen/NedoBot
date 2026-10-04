use crate::config::Config;

pub fn should_generate_comment(post_text: &str, config: &Config) -> bool {
    should_generate_comment_with_marker(post_text, &config.post_signature_marker)
}

pub fn clean_post_for_llm(post_text: &str, config: &Config) -> String {
    clean_post_for_llm_with_marker(post_text, &config.post_signature_marker)
}

pub fn should_generate_comment_with_marker(post_text: &str, marker: &str) -> bool {
    should_generate_comment_with_markers(post_text, &[marker.to_owned()])
}

pub fn should_generate_comment_with_markers(post_text: &str, markers: &[String]) -> bool {
    markers
        .iter()
        .any(|marker| !marker.is_empty() && post_text.contains(marker))
}

pub fn clean_post_for_llm_with_marker(post_text: &str, marker: &str) -> String {
    clean_post_for_llm_with_markers(post_text, &[marker.to_owned()])
}

pub fn clean_post_for_llm_with_markers(post_text: &str, markers: &[String]) -> String {
    let cut = markers
        .iter()
        .filter(|marker| !marker.is_empty())
        .filter_map(|marker| post_text.find(marker))
        .min();
    let without_signature = match cut {
        Some(index) => &post_text[..index],
        None => post_text,
    };

    without_signature.trim().to_string()
}

/// First matching denylist term (case-insensitive) or `None`. Empty terms
/// never match so config typos fail open toward commenting.
pub fn blocked_post_term(post_text: &str, blocked_terms: &[String]) -> Option<String> {
    let lower = post_text.to_lowercase();
    blocked_terms.iter().find_map(|term| {
        let term = term.trim().to_lowercase();
        (!term.is_empty() && lower.contains(&term)).then(|| term.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD_MARKER: &str = "Не теряем связь";
    const NEW_MARKER: &str = "😎НедоNews";

    fn markers() -> Vec<String> {
        vec![OLD_MARKER.to_string(), NEW_MARKER.to_string()]
    }

    #[test]
    fn new_editorial_footer_passes_gate_and_is_stripped() {
        let post = "Вышел патч 1.1.0 с выбором вызова.\n\n😎НедоNews";
        assert!(should_generate_comment_with_markers(post, &markers()));
        assert_eq!(
            clean_post_for_llm_with_markers(post, &markers()),
            "Вышел патч 1.1.0 с выбором вызова."
        );
    }

    #[test]
    fn retired_footer_still_passes_gate_and_is_stripped() {
        let post = "Вышел патч 1.1.0.\n\nНе теряем связь: нас можно читать в МАКС";
        assert!(should_generate_comment_with_markers(post, &markers()));
        assert_eq!(
            clean_post_for_llm_with_markers(post, &markers()),
            "Вышел патч 1.1.0."
        );
    }

    #[test]
    fn earliest_marker_wins_when_both_present() {
        let post = "Новость.\n\n😎НедоNews\n\nНе теряем связь";
        assert_eq!(
            clean_post_for_llm_with_markers(post, &markers()),
            "Новость."
        );
    }

    #[test]
    fn ads_without_footer_stay_out() {
        let post = "Зарабатывайте на установках Яндекс Браузера\n\n#реклама 0+";
        assert!(!should_generate_comment_with_markers(post, &markers()));
        assert_eq!(
            clean_post_for_llm_with_markers(post, &markers()),
            post.trim()
        );
    }

    #[test]
    fn empty_markers_never_match() {
        assert!(!should_generate_comment_with_markers(
            "Новость 😎НедоNews",
            &[]
        ));
        assert!(!should_generate_comment_with_markers("Новость", &markers()));
    }

    #[test]
    fn blocked_terms_veto_even_with_footer() {
        let blocked = vec!["#реклама".to_string(), "О рекламодателе".to_string()];
        let ad = "Встречай смартфон Honor\n\n😎НедоNews\n#реклама";
        assert!(should_generate_comment_with_markers(ad, &markers()));
        assert_eq!(
            blocked_post_term(ad, &blocked),
            Some("#реклама".to_string())
        );
        assert_eq!(
            blocked_post_term("Вышел патч 1.1.0.\n\n😎НедоNews", &blocked),
            None
        );
        assert_eq!(blocked_post_term("Вышел патч.", &[]), None);
        // Empty config entries never match.
        assert_eq!(blocked_post_term("Вышел патч.", &["   ".to_string()]), None);
    }
}
