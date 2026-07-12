use serde::Deserialize;

use crate::features::first_comment::quality::validate_comment_output;

/// Structured LLM output for a first-comment generation.
///
/// Keeping provenance separate from the visible comment lets the renderer and
/// the database stay in control of external links and follow-up analytics.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FirstCommentDraft {
    pub comment: String,
    pub used_search_result_id: Option<usize>,
}

pub fn parse_first_comment_draft(value: &str) -> anyhow::Result<FirstCommentDraft> {
    let draft: FirstCommentDraft = serde_json::from_str(value)
        .map_err(|err| anyhow::anyhow!("first comment response must be a JSON object: {err}"))?;

    if draft.used_search_result_id == Some(0) {
        anyhow::bail!("used_search_result_id must start at 1");
    }

    Ok(draft)
}

pub fn validate_first_comment_draft_output(value: &str) -> anyhow::Result<()> {
    let draft = parse_first_comment_draft(value)?;
    validate_comment_output(&draft.comment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comment_and_search_provenance() {
        let draft = parse_first_comment_draft(
            r#"{"comment":"Память снова дорожает. Прайсы в {CHAT_LINK:чатике}","used_search_result_id":2}"#,
        )
        .unwrap();

        assert_eq!(draft.used_search_result_id, Some(2));
        assert_eq!(
            draft.comment,
            "Память снова дорожает. Прайсы в {CHAT_LINK:чатике}"
        );
    }

    #[test]
    fn rejects_markdown_wrapped_json() {
        assert!(parse_first_comment_draft(
            "```json\n{\"comment\":\"Память дорожает. Прайсы в {CHAT_LINK}\",\"used_search_result_id\":null}\n```",
        )
        .is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(parse_first_comment_draft(
            r#"{"comment":"Память дорожает. Прайсы в {CHAT_LINK}","used_search_result_id":null,"thought":"..."}"#,
        )
        .is_err());
    }

    #[test]
    fn rejects_zero_search_result_id() {
        assert!(
            parse_first_comment_draft(
                r#"{"comment":"Память дорожает. Прайсы в {CHAT_LINK}","used_search_result_id":0}"#,
            )
            .is_err()
        );
    }

    #[test]
    fn validator_checks_visible_comment() {
        validate_first_comment_draft_output(
            r#"{"comment":"Память дорожает, а заводы не успевают. Прайсы в {CHAT_LINK:чатике}","used_search_result_id":null}"#,
        )
        .unwrap();
    }
}
