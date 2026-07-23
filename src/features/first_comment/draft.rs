use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::Config;
use crate::features::first_comment::prompt::search_result_source_name;
use crate::features::first_comment::quality::validate_comment_output;
use crate::features::search::mcp::is_safe_fetch_url;
use crate::features::search::policy::{is_allowed_comment_text, is_allowed_source_url};
use crate::features::search::types::SearchResult;

const GENERIC_SOURCE_LINK_LABELS: &[&str] = &[
    "детали",
    "подробнее",
    "источник",
    "ссылка",
    "здесь",
    "тут",
    "пруф",
];

/// Structured LLM output for a first-comment generation.
///
/// Keeping provenance separate from the visible comment lets the renderer and
/// the database stay in control of external links and follow-up analytics.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FirstCommentDraft {
    pub comment: String,
    pub used_search_result_id: Option<usize>,
    #[serde(default)]
    pub used_chat_message_ids: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLinkPlaceholder {
    pub result_id: usize,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatEvidencePlaceholder {
    pub message_id: i32,
    /// `None` means the renderer must take the display name from the confirmed DB target.
    pub message_label: Option<String>,
}

static FIRST_COMMENT_OUTPUT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "comment": {
                "type": "string",
                "description": "Visible Russian first comment with one CHAT_LINK placeholder."
            },
            "used_search_result_id": {
                "type": ["integer", "null"],
                "description": "One-based ID of the search result used for a new factual addition, or null."
            },
            "used_chat_message_ids": {
                "type": "array", "maxItems": 3,
                "items": { "type": "integer", "minimum": 1 }
            }
        },
        "required": ["comment", "used_search_result_id", "used_chat_message_ids"],
        "additionalProperties": false
    })
});

pub fn first_comment_output_schema() -> &'static Value {
    &FIRST_COMMENT_OUTPUT_SCHEMA
}

pub fn parse_first_comment_draft(value: &str) -> anyhow::Result<FirstCommentDraft> {
    let trimmed = value.trim();
    let draft = match serde_json::from_str(trimmed) {
        Ok(draft) => draft,
        Err(err) if looks_like_structured_output(trimmed) => {
            anyhow::bail!("first comment response must be a JSON object: {err}")
        }
        Err(_) => FirstCommentDraft {
            comment: trimmed.to_string(),
            used_search_result_id: None,
            used_chat_message_ids: Vec::new(),
        },
    };

    if draft.used_search_result_id == Some(0) {
        anyhow::bail!("used_search_result_id must start at 1");
    }
    if draft.used_chat_message_ids.len() > 3
        || draft.used_chat_message_ids.iter().any(|id| *id <= 0)
    {
        anyhow::bail!("used_chat_message_ids must contain at most three positive IDs");
    }

    Ok(draft)
}

fn looks_like_structured_output(value: &str) -> bool {
    value.starts_with('{') || value.starts_with('[') || value.starts_with("```")
}

#[allow(dead_code)] // Compatibility helper retained for focused validator tests.
pub fn validate_first_comment_draft_with_search(
    value: &str,
    search_results: &[SearchResult],
    source_link_available: bool,
) -> anyhow::Result<()> {
    validate_first_comment_draft_with_search_and_policy(
        value,
        search_results,
        source_link_available,
        &Config::from_env(),
    )
}

#[allow(dead_code)] // Compatibility helper retained for callers without chat evidence.
pub fn validate_first_comment_draft_with_search_and_policy(
    value: &str,
    search_results: &[SearchResult],
    source_link_available: bool,
    config: &Config,
) -> anyhow::Result<()> {
    validate_first_comment_draft_with_search_policy_and_chat(
        value,
        search_results,
        source_link_available,
        config,
        &[],
    )
}

pub fn validate_first_comment_draft_with_search_policy_and_chat(
    value: &str,
    search_results: &[SearchResult],
    source_link_available: bool,
    config: &Config,
    allowed_chat_message_ids: &[i32],
) -> anyhow::Result<()> {
    let draft = parse_first_comment_draft(value)?;
    let source_link = validate_comment_body(&draft, config, allowed_chat_message_ids)?;

    if let Some(result_id) = draft.used_search_result_id {
        let result = search_result_by_id(search_results, result_id)?;
        if source_link.is_some()
            && (!is_safe_fetch_url(&result.url) || !is_allowed_source_url(config, &result.url))
        {
            anyhow::bail!("selected source link is not allowed by source policy");
        }
    }

    if draft.used_search_result_id.is_some() && source_link.is_none() {
        anyhow::bail!("used search result must have a SOURCE_LINK");
    }

    if let Some(source_link) = source_link {
        if !source_link_available {
            anyhow::bail!("SOURCE_LINK is disabled for this comment");
        }
        if draft.used_search_result_id != Some(source_link.result_id) {
            anyhow::bail!("SOURCE_LINK result ID must match used_search_result_id");
        }
        let result = search_result_by_id(search_results, source_link.result_id)?;
        if !is_safe_fetch_url(&result.url) || !is_allowed_source_url(config, &result.url) {
            anyhow::bail!("selected source link is not allowed by source policy");
        }
        let source_name = search_result_source_name(result);
        if !source_link
            .label
            .to_lowercase()
            .contains(&source_name.to_lowercase())
        {
            anyhow::bail!("SOURCE_LINK label must name the linked source: {source_name}");
        }
    }

    Ok(())
}

fn validate_comment_body(
    draft: &FirstCommentDraft,
    config: &Config,
    allowed_chat_message_ids: &[i32],
) -> anyhow::Result<Option<SourceLinkPlaceholder>> {
    let (visible_comment, source_link) = replace_source_link_placeholder(&draft.comment)?;
    let (visible_comment, evidence_ids) = replace_chat_evidence_placeholders(&visible_comment)?;
    if evidence_ids != draft.used_chat_message_ids {
        anyhow::bail!("used_chat_message_ids must exactly match chat evidence placeholders");
    }
    if evidence_ids.is_empty() {
        validate_comment_output(&visible_comment)?;
    } else {
        if evidence_ids.len() > 3
            || evidence_ids
                .iter()
                .any(|id| !allowed_chat_message_ids.contains(id))
            || draft.used_chat_message_ids != evidence_ids
        {
            anyhow::bail!("chat evidence IDs do not match the confirmed retrieval context");
        }
        validate_comment_output(&format!("{visible_comment} {{CHAT_LINK}}"))?;
    }
    if !is_allowed_comment_text(config, &visible_comment) {
        anyhow::bail!("first comment contains a blocked term");
    }
    Ok(source_link)
}

fn replace_chat_evidence_placeholders(text: &str) -> anyhow::Result<(String, Vec<i32>)> {
    let mut visible = String::with_capacity(text.len());
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(start) = [rest.find("{CHAT_AUTHOR"), rest.find("{CHAT_MESSAGE")]
        .into_iter()
        .flatten()
        .min()
    {
        let (before, after_start) = rest.split_at(start);
        visible.push_str(before);
        let Some(end) = after_start.find('}') else {
            anyhow::bail!("unterminated chat evidence placeholder")
        };
        let token = &after_start[..=end];
        let placeholder = parse_chat_evidence_placeholder(token)?;
        let id = placeholder.message_id;
        if !ids.contains(&id) {
            ids.push(id);
        }
        visible.push_str(placeholder.message_label.as_deref().unwrap_or(" "));
        rest = &after_start[end + 1..];
    }
    visible.push_str(rest);
    Ok((visible, ids))
}

pub fn parse_chat_evidence_placeholder(token: &str) -> anyhow::Result<ChatEvidencePlaceholder> {
    if let Some(value) = token
        .strip_prefix("{CHAT_AUTHOR:")
        .and_then(|value| value.strip_suffix('}'))
    {
        let message_id = value
            .parse::<i32>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| anyhow::anyhow!("malformed chat evidence placeholder: {token}"))?;
        return Ok(ChatEvidencePlaceholder {
            message_id,
            message_label: None,
        });
    }

    let value = token
        .strip_prefix("{CHAT_MESSAGE:")
        .and_then(|value| value.strip_suffix('}'))
        .ok_or_else(|| anyhow::anyhow!("malformed chat evidence placeholder: {token}"))?;
    let (id, label) = value
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("CHAT_MESSAGE must contain message ID and label"))?;
    let message_id = id
        .parse::<i32>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| anyhow::anyhow!("malformed chat evidence placeholder: {token}"))?;
    let label = label.trim();
    if label.is_empty()
        || label.chars().count() > 40
        || !label
            .chars()
            .all(|ch| ch.is_alphanumeric() || ch.is_whitespace() || ch == '-')
    {
        anyhow::bail!("CHAT_MESSAGE label contains unsupported characters");
    }
    Ok(ChatEvidencePlaceholder {
        message_id,
        message_label: Some(label.to_string()),
    })
}

fn search_result_by_id(
    results: &[SearchResult],
    result_id: usize,
) -> anyhow::Result<&SearchResult> {
    results.get(result_id - 1).ok_or_else(|| {
        anyhow::anyhow!("used_search_result_id does not exist in this search context")
    })
}

pub fn parse_source_link_placeholder(token: &str) -> anyhow::Result<SourceLinkPlaceholder> {
    let value = token
        .strip_prefix("{SOURCE_LINK:")
        .and_then(|value| value.strip_suffix('}'))
        .ok_or_else(|| anyhow::anyhow!("malformed SOURCE_LINK placeholder: {token}"))?;
    let (result_id, label) = value
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("SOURCE_LINK must contain result ID and label"))?;
    let result_id = result_id
        .trim()
        .parse::<usize>()
        .map_err(|_| anyhow::anyhow!("SOURCE_LINK result ID must be a positive integer"))?;
    if result_id == 0 {
        anyhow::bail!("SOURCE_LINK result ID must start at 1");
    }

    let label = label.trim();
    if label.is_empty() || label.chars().count() > 40 {
        anyhow::bail!("SOURCE_LINK label must contain 1 to 40 characters");
    }
    if !label
        .chars()
        .all(|ch| ch.is_alphanumeric() || ch.is_whitespace() || matches!(ch, '-' | '+'))
    {
        anyhow::bail!("SOURCE_LINK label contains unsupported characters");
    }
    if GENERIC_SOURCE_LINK_LABELS.contains(&label.to_lowercase().as_str()) {
        anyhow::bail!("SOURCE_LINK label must be part of the sentence, not a generic pointer");
    }

    Ok(SourceLinkPlaceholder {
        result_id,
        label: label.to_string(),
    })
}

fn replace_source_link_placeholder(
    text: &str,
) -> anyhow::Result<(String, Option<SourceLinkPlaceholder>)> {
    let mut visible = String::with_capacity(text.len());
    let mut source_link = None;
    let mut rest = text;

    while let Some(start) = rest.find("{SOURCE_LINK") {
        let (before, after_start) = rest.split_at(start);
        visible.push_str(before);
        let Some(end) = after_start.find('}') else {
            anyhow::bail!("unterminated SOURCE_LINK placeholder");
        };
        if source_link.is_some() {
            anyhow::bail!("first comment contains multiple SOURCE_LINK placeholders");
        }

        let placeholder = parse_source_link_placeholder(&after_start[..=end])?;
        visible.push_str(&placeholder.label);
        source_link = Some(placeholder);
        rest = &after_start[end + 1..];
    }

    visible.push_str(rest);
    Ok((visible, source_link))
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
    fn falls_back_to_legacy_plain_comment() {
        let draft = parse_first_comment_draft(
            "Память дорожает, а заводы не успевают. Прайсы в {CHAT_LINK:чатике}",
        )
        .unwrap();

        assert_eq!(draft.used_search_result_id, None);
        assert!(draft.comment.contains("{CHAT_LINK:чатике}"));
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
        let draft = parse_first_comment_draft(
            r#"{"comment":"Память дорожает, а заводы не успевают. Прайсы в {CHAT_LINK:чатике}","used_search_result_id":null}"#,
        )
        .unwrap();

        validate_comment_body(&draft, &Config::from_env(), &[]).unwrap();
    }

    #[test]
    fn chat_evidence_can_replace_invite_link_only_for_allowed_id() {
        validate_first_comment_draft_with_search_policy_and_chat(
            r#"{"comment":"{CHAT_AUTHOR:42} разбирал TPM, похожую боль можно продолжить здесь","used_search_result_id":null,"used_chat_message_ids":[42]}"#,
            &[],
            false,
            &Config::from_env(),
            &[42],
        )
        .unwrap();
    }

    #[test]
    fn rejects_chat_evidence_outside_retrieval_context() {
        assert!(validate_first_comment_draft_with_search_policy_and_chat(
            r#"{"comment":"{CHAT_AUTHOR:99} разбирал TPM, похожую боль можно продолжить здесь","used_search_result_id":null,"used_chat_message_ids":[99]}"#,
            &[], false, &Config::from_env(), &[42],
        ).is_err());
    }

    #[test]
    fn rejects_claimed_chat_provenance_without_placeholder() {
        assert!(validate_first_comment_draft_with_search_policy_and_chat(
            r#"{"comment":"TPM всё ещё горячая тема в {CHAT_LINK:чатике}","used_search_result_id":null,"used_chat_message_ids":[42]}"#,
            &[], false, &Config::from_env(), &[42],
        )
        .is_err());
    }

    #[test]
    fn rejects_chat_message_label_the_renderer_would_not_link() {
        assert!(validate_first_comment_draft_with_search_policy_and_chat(
            r#"{"comment":"Похожее обсуждение было в {CHAT_MESSAGE:42:<script>}","used_search_result_id":null,"used_chat_message_ids":[42]}"#,
            &[], false, &Config::from_env(), &[42],
        )
        .is_err());
    }

    #[test]
    fn schema_requires_comment_and_search_provenance() {
        let schema = first_comment_output_schema();
        assert_eq!(
            schema["required"],
            serde_json::json!(["comment", "used_search_result_id", "used_chat_message_ids"])
        );
        assert_eq!(
            schema["properties"]["used_search_result_id"]["type"],
            serde_json::json!(["integer", "null"])
        );
        assert_eq!(schema["properties"]["used_chat_message_ids"]["maxItems"], 3);
    }

    #[test]
    fn validates_source_link_against_matching_search_result() {
        let results = vec![SearchResult {
            source: crate::features::search::types::SearchSource::Web,
            title: "Court decision".to_string(),
            url: "https://example.com/court".to_string(),
            snippet: "Court restored the account.".to_string(),
        }];

        validate_first_comment_draft_with_search(
            r#"{"comment":"Судя по {SOURCE_LINK:1:решению суда Example}, аккаунт вернули только после иска. Поддержка Xbox в {CHAT_LINK:чатике}","used_search_result_id":1}"#,
            &results,
            true,
        )
        .unwrap();
    }

    #[test]
    fn rejects_source_link_with_wrong_provenance() {
        let results = vec![SearchResult {
            source: crate::features::search::types::SearchSource::Web,
            title: "Court decision".to_string(),
            url: "https://example.com/court".to_string(),
            snippet: String::new(),
        }];

        assert!(validate_first_comment_draft_with_search(
            r#"{"comment":"Судя по {SOURCE_LINK:1:решению суда Example}, аккаунт вернули только после иска. Поддержка Xbox в {CHAT_LINK:чатике}","used_search_result_id":null}"#,
            &results,
            true,
        )
        .is_err());
    }

    #[test]
    fn rejects_missing_source_link_when_search_result_is_used() {
        let results = vec![SearchResult {
            source: crate::features::search::types::SearchSource::Web,
            title: "Release notes".to_string(),
            url: "https://example.com/release".to_string(),
            snippet: "Version 2.0 is available.".to_string(),
        }];

        assert!(
            validate_first_comment_draft_with_search(
                r#"{"comment":"Версия 2.0 уже вышла. Детали в {CHAT_LINK:чатике}","used_search_result_id":1}"#,
                &results,
                true,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_generic_source_link_label() {
        let results = vec![SearchResult {
            source: crate::features::search::types::SearchSource::Web,
            title: "Release notes".to_string(),
            url: "https://example.com/release".to_string(),
            snippet: "Version 2.0 is available.".to_string(),
        }];

        assert!(validate_first_comment_draft_with_search(
            r#"{"comment":"Версия 2.0 уже вышла. {SOURCE_LINK:1:Детали} в {CHAT_LINK:чатике}","used_search_result_id":1}"#,
            &results,
            true,
        )
        .is_err());
    }

    #[test]
    fn accepts_source_link_as_part_of_sentence() {
        let results = vec![SearchResult {
            source: crate::features::search::types::SearchSource::Web,
            title: "Release notes".to_string(),
            url: "https://example.com/release".to_string(),
            snippet: "Version 2.0 is available.".to_string(),
        }];

        validate_first_comment_draft_with_search(
            r#"{"comment":"Как пишет {SOURCE_LINK:1:ресурс Example}, версия уже вышла. Обновление в {CHAT_LINK:чатике}","used_search_result_id":1}"#,
            &results,
            true,
        )
        .unwrap();
    }

    #[test]
    fn rejects_source_label_that_does_not_match_link_domain() {
        let results = vec![SearchResult {
            source: crate::features::search::types::SearchSource::Reddit,
            title: "Marketplace post".to_string(),
            url: "https://amazon.com/example".to_string(),
            snippet: String::new(),
        }];

        assert!(validate_first_comment_draft_with_search(
            r#"{"comment":"Как пишет {SOURCE_LINK:1:Reddit}, товар уже появился. Покупки в {CHAT_LINK:чатике}","used_search_result_id":1}"#,
            &results,
            true,
        )
        .is_err());
    }

    #[test]
    fn allows_ignoring_search_context_when_it_does_not_add_scope() {
        let results = vec![SearchResult {
            source: crate::features::search::types::SearchSource::Web,
            title: "Release notes".to_string(),
            url: "https://example.com/release".to_string(),
            snippet: "Version 2.0 is available.".to_string(),
        }];

        validate_first_comment_draft_with_search(
            r#"{"comment":"Обновление уже вышло. Детали в {CHAT_LINK:чатике}","used_search_result_id":null}"#,
            &results,
            true,
        )
        .unwrap();
    }
}
