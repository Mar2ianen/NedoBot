use crate::config::Config;
use crate::features::search::types::SearchResult;

pub fn is_allowed_search_result(config: &Config, result: &SearchResult) -> bool {
    is_allowed_source_url(config, &result.url)
        && is_allowed_text(config, &result.title)
        && is_allowed_text(config, &result.snippet)
}

pub fn is_allowed_source_url(config: &Config, value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return true;
    };
    let Some(host) = url.host_str() else {
        return true;
    };
    let host = normalize_domain(host);

    !config
        .comment_blocked_source_domains
        .iter()
        .map(|domain| normalize_domain(domain))
        .any(|domain| host == domain || host.ends_with(&format!(".{domain}")))
}

pub fn is_allowed_comment_text(config: &Config, text: &str) -> bool {
    is_allowed_text(config, text)
}

fn is_allowed_text(config: &Config, text: &str) -> bool {
    let lower = text.to_lowercase();
    !config.comment_blocked_terms.iter().any(|term| {
        let term = term.trim().to_lowercase();
        !term.is_empty() && lower.contains(&term)
    })
}

fn normalize_domain(value: &str) -> String {
    value.trim().trim_end_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_exact_domain_and_subdomains_without_overmatching() {
        let mut config = crate::config::test_config();
        config.comment_blocked_source_domains = vec!["meduza.io".to_string()];

        assert!(!is_allowed_source_url(
            &config,
            "https://meduza.io/en/story"
        ));
        assert!(!is_allowed_source_url(
            &config,
            "https://www.meduza.io/en/story"
        ));
        assert!(is_allowed_source_url(&config, "https://notmeduza.io/story"));
    }

    #[test]
    fn configured_policy_blocks_multiple_sources() {
        let config = crate::config::test_config();
        assert!(!config.comment_blocked_source_domains.is_empty());

        config
            .comment_blocked_source_domains
            .iter()
            .for_each(|domain| {
                assert!(
                    !is_allowed_source_url(&config, &format!("https://{domain}/story")),
                    "configured source domain {domain} must be blocked"
                );
            });
        assert!(!is_allowed_source_url(
            &config,
            "https://www.currenttime.tv/story"
        ));
    }

    #[test]
    fn blocks_configured_terms_in_comment_text() {
        let mut config = crate::config::test_config();
        config.comment_blocked_terms = vec!["запрещенное название".to_string()];

        assert!(!is_allowed_comment_text(
            &config,
            "Ссылка ведёт на запрещенное название",
        ));
        assert!(is_allowed_comment_text(&config, "Обычный комментарий"));
    }
}
