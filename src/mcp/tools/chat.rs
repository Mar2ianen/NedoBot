//! Typed semantic chat tools backed by `ChatReadApi`.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sqlx::types::chrono::{DateTime, Utc};

use crate::features::chat_read_api::{
    ChatReadApi,
    types::{MessageMatch, MessageSearchRequest, MessageSort, RecentMessagesRequest},
};

use super::{invalid_arguments, read_error};

const DEFAULT_SEARCH_LIMIT: i64 = 10;
const DEFAULT_RECENT_LIMIT: i64 = 20;
const DEFAULT_CONTEXT_WINDOW: i64 = 3;
const MAX_BATCH_QUERIES: usize = 6;
const MAX_BATCH_LIMIT: i64 = 5;

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    FullText,
    Literal,
}

impl From<MatchMode> for MessageMatch {
    fn from(value: MatchMode) -> Self {
        match value {
            MatchMode::FullText => Self::FullText,
            MatchMode::Literal => Self::Literal,
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    Relevance,
    Newest,
    Oldest,
}

impl From<Sort> for MessageSort {
    fn from(value: Sort) -> Self {
        match value {
            Sort::Relevance => Self::Relevance,
            Sort::Newest => Self::Newest,
            Sort::Oldest => Self::Oldest,
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchMessagesInput {
    pub query: String,
    pub user_id: Option<i64>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub reply_to_message_id: Option<i32>,
    pub has_links: Option<bool>,
    pub has_media: Option<bool>,
    pub match_mode: Option<MatchMode>,
    pub sort: Option<Sort>,
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchMessagesBatchInput {
    pub queries: Vec<String>,
    pub user_id: Option<i64>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub has_links: Option<bool>,
    pub has_media: Option<bool>,
    pub match_mode: Option<MatchMode>,
    pub sort: Option<Sort>,
    pub limit_per_query: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecentMessagesInput {
    pub user_id: Option<i64>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub has_links: Option<bool>,
    pub has_media: Option<bool>,
    pub sort: Option<Sort>,
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageContextInput {
    pub message_id: i32,
    pub before: Option<i64>,
    pub after: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserInteractionsInput {
    pub first_user_id: i64,
    pub second_user_id: i64,
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserProfileInput {
    pub telegram_user_id: i64,
}

#[derive(Serialize, JsonSchema)]
pub struct BatchSearchResult {
    pub query: String,
    pub messages: serde_json::Value,
}

fn parse_timestamp(value: Option<String>) -> Result<Option<DateTime<Utc>>, rmcp::ErrorData> {
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|_| invalid_arguments("timestamps must be RFC 3339"))
        })
        .transpose()
}

fn search_request(input: SearchMessagesInput) -> Result<MessageSearchRequest, rmcp::ErrorData> {
    if input.query.trim().is_empty() {
        return Err(invalid_arguments("query must not be empty"));
    }
    Ok(MessageSearchRequest {
        query: input.query,
        user_id: input.user_id,
        date_from: parse_timestamp(input.date_from)?,
        date_to: parse_timestamp(input.date_to)?,
        reply_to_message_id: input.reply_to_message_id,
        has_links: input.has_links,
        has_media: input.has_media,
        match_mode: input.match_mode.unwrap_or(MatchMode::FullText).into(),
        sort: input.sort.unwrap_or(Sort::Relevance).into(),
        limit: input.limit.unwrap_or(DEFAULT_SEARCH_LIMIT),
    })
}

pub async fn search_messages(
    api: &ChatReadApi,
    input: SearchMessagesInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let messages = api
        .search_messages(&search_request(input)?)
        .await
        .map_err(|_| read_error("chat search failed"))?;
    serde_json::to_value(messages).map_err(|_| read_error("cannot encode chat result"))
}

pub async fn search_messages_batch(
    api: &ChatReadApi,
    input: SearchMessagesBatchInput,
) -> Result<Vec<BatchSearchResult>, rmcp::ErrorData> {
    if input.queries.is_empty() {
        return Err(invalid_arguments("queries must not be empty"));
    }
    let date_from = parse_timestamp(input.date_from)?;
    let date_to = parse_timestamp(input.date_to)?;
    let limit = input
        .limit_per_query
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_BATCH_LIMIT);
    let mut results = Vec::new();
    for query in input.queries.into_iter().take(MAX_BATCH_QUERIES) {
        let messages = api
            .search_messages(&MessageSearchRequest {
                query: query.clone(),
                user_id: input.user_id,
                date_from,
                date_to,
                reply_to_message_id: None,
                has_links: input.has_links,
                has_media: input.has_media,
                match_mode: input
                    .match_mode
                    .clone()
                    .unwrap_or(MatchMode::FullText)
                    .into(),
                sort: input.sort.clone().unwrap_or(Sort::Relevance).into(),
                limit,
            })
            .await
            .map_err(|_| read_error("chat batch search failed"))?;
        results.push(BatchSearchResult {
            query,
            messages: serde_json::to_value(messages)
                .map_err(|_| read_error("cannot encode chat result"))?,
        });
    }
    Ok(results)
}

pub async fn recent_messages(
    api: &ChatReadApi,
    input: RecentMessagesInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let messages = api
        .recent_messages(&RecentMessagesRequest {
            user_id: input.user_id,
            date_from: parse_timestamp(input.date_from)?,
            date_to: parse_timestamp(input.date_to)?,
            has_links: input.has_links,
            has_media: input.has_media,
            sort: input.sort.unwrap_or(Sort::Newest).into(),
            limit: input.limit.unwrap_or(DEFAULT_RECENT_LIMIT),
        })
        .await
        .map_err(|_| read_error("recent message lookup failed"))?;
    serde_json::to_value(messages).map_err(|_| read_error("cannot encode chat result"))
}

pub async fn get_message(
    api: &ChatReadApi,
    input: MessageContextInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    context(api, input.message_id, 0, 0).await
}

pub async fn message_context(
    api: &ChatReadApi,
    input: MessageContextInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    context(
        api,
        input.message_id,
        input.before.unwrap_or(DEFAULT_CONTEXT_WINDOW),
        input.after.unwrap_or(DEFAULT_CONTEXT_WINDOW),
    )
    .await
}

async fn context(
    api: &ChatReadApi,
    message_id: i32,
    before: i64,
    after: i64,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let messages = api
        .message_context(message_id, before, after)
        .await
        .map_err(|_| read_error("message context lookup failed"))?;
    serde_json::to_value(messages).map_err(|_| read_error("cannot encode chat result"))
}

pub async fn reply_thread(
    api: &ChatReadApi,
    input: MessageContextInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let messages = api
        .reply_thread(input.message_id)
        .await
        .map_err(|_| read_error("reply thread lookup failed"))?;
    serde_json::to_value(messages).map_err(|_| read_error("cannot encode chat result"))
}

pub async fn user_interactions(
    api: &ChatReadApi,
    input: UserInteractionsInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    if input.first_user_id == input.second_user_id {
        return Err(invalid_arguments("users must be different"));
    }
    let interactions = api
        .user_interactions(
            input.first_user_id,
            input.second_user_id,
            input.limit.unwrap_or(DEFAULT_RECENT_LIMIT),
        )
        .await
        .map_err(|_| read_error("user interaction lookup failed"))?;
    serde_json::to_value(interactions).map_err(|_| read_error("cannot encode chat result"))
}

pub async fn user_profile(
    api: &ChatReadApi,
    input: UserProfileInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let profile = api
        .user_profile(input.telegram_user_id)
        .await
        .map_err(|_| read_error("user profile lookup failed"))?;
    Ok(serde_json::json!({"found": profile.is_some(), "profile": profile}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_input_rejects_unknown_fields() {
        assert!(
            serde_json::from_value::<SearchMessagesInput>(serde_json::json!({
                "query": "rust",
                "unexpected": true,
            }))
            .is_err()
        );
    }

    #[test]
    fn invalid_search_is_a_tool_error_before_database_access() {
        let error = search_request(SearchMessagesInput {
            query: "  ".into(),
            user_id: None,
            date_from: None,
            date_to: None,
            reply_to_message_id: None,
            has_links: None,
            has_media: None,
            match_mode: None,
            sort: None,
            limit: None,
        })
        .unwrap_err();
        assert_eq!(error.message, "query must not be empty");
    }
}
