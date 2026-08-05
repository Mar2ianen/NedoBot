//! Typed semantic chat tools backed by `ChatReadApi`.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sqlx::types::chrono::{DateTime, Utc};

use crate::features::chat_read_api::{
    ChatReadApi,
    service::MAX_SEARCH_OFFSET,
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
    Hybrid,
    FullText,
    AnyTerms,
    Literal,
    WholeWord,
}

impl From<MatchMode> for MessageMatch {
    fn from(value: MatchMode) -> Self {
        match value {
            MatchMode::Hybrid => Self::Hybrid,
            MatchMode::FullText => Self::FullText,
            MatchMode::AnyTerms => Self::AnyTerms,
            MatchMode::Literal => Self::Literal,
            MatchMode::WholeWord => Self::WholeWord,
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
    pub is_automatic_forward: Option<bool>,
    pub has_reply: Option<bool>,
    pub has_links: Option<bool>,
    pub has_media: Option<bool>,
    pub has_photo: Option<bool>,
    pub has_video: Option<bool>,
    pub has_document: Option<bool>,
    pub has_audio: Option<bool>,
    pub has_voice: Option<bool>,
    pub has_sticker: Option<bool>,
    pub has_animation: Option<bool>,
    pub match_mode: Option<MatchMode>,
    pub sort: Option<Sort>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    #[serde(default)]
    pub include_forwards: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchMessagesBatchInput {
    pub queries: Vec<String>,
    pub user_id: Option<i64>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub is_automatic_forward: Option<bool>,
    pub has_reply: Option<bool>,
    pub has_links: Option<bool>,
    pub has_media: Option<bool>,
    pub has_photo: Option<bool>,
    pub has_video: Option<bool>,
    pub has_document: Option<bool>,
    pub has_audio: Option<bool>,
    pub has_voice: Option<bool>,
    pub has_sticker: Option<bool>,
    pub has_animation: Option<bool>,
    pub match_mode: Option<MatchMode>,
    pub sort: Option<Sort>,
    pub limit_per_query: Option<i64>,
    pub offset: Option<i64>,
    #[serde(default)]
    pub include_forwards: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CountMessagesInput {
    /// Optional text predicate. Omit it to count all messages matching the
    /// structural filters, for example all messages of one user.
    pub query: Option<String>,
    pub user_id: Option<i64>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub reply_to_message_id: Option<i32>,
    pub is_automatic_forward: Option<bool>,
    pub has_reply: Option<bool>,
    pub has_links: Option<bool>,
    pub has_media: Option<bool>,
    pub has_photo: Option<bool>,
    pub has_video: Option<bool>,
    pub has_document: Option<bool>,
    pub has_audio: Option<bool>,
    pub has_voice: Option<bool>,
    pub has_sticker: Option<bool>,
    pub has_animation: Option<bool>,
    pub match_mode: Option<MatchMode>,
    #[serde(default)]
    pub include_forwards: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecentMessagesInput {
    pub user_id: Option<i64>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub is_automatic_forward: Option<bool>,
    pub has_reply: Option<bool>,
    pub has_links: Option<bool>,
    pub has_media: Option<bool>,
    pub has_photo: Option<bool>,
    pub has_video: Option<bool>,
    pub has_document: Option<bool>,
    pub has_audio: Option<bool>,
    pub has_voice: Option<bool>,
    pub has_sticker: Option<bool>,
    pub has_animation: Option<bool>,
    pub sort: Option<Sort>,
    pub limit: Option<i64>,
    #[serde(default)]
    pub include_forwards: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageIdInput {
    pub message_id: i32,
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
    pub total_count: i64,
    pub has_more: bool,
    pub next_offset: Option<i64>,
    pub scan_limit_reached: bool,
}

#[derive(Clone, Copy)]
enum DateBoundary {
    Start,
    End,
}

fn parse_timestamp(
    value: Option<String>,
    boundary: DateBoundary,
) -> Result<Option<DateTime<Utc>>, rmcp::ErrorData> {
    value
        .map(|value| parse_timestamp_value(&value, boundary))
        .transpose()
}

fn parse_timestamp_value(
    value: &str,
    boundary: DateBoundary,
) -> Result<DateTime<Utc>, rmcp::ErrorData> {
    if let Ok(value) = DateTime::parse_from_rfc3339(value) {
        return Ok(value.with_timezone(&Utc));
    }

    let date = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| invalid_arguments("timestamps must be RFC 3339 or YYYY-MM-DD"))?;
    let time = match boundary {
        DateBoundary::Start => chrono::NaiveTime::MIN,
        DateBoundary::End => chrono::NaiveTime::from_hms_micro_opt(23, 59, 59, 999_999)
            .expect("valid end-of-day time"),
    };
    Ok(DateTime::<Utc>::from_naive_utc_and_offset(
        date.and_time(time),
        Utc,
    ))
}

fn search_request(input: SearchMessagesInput) -> Result<MessageSearchRequest, rmcp::ErrorData> {
    if input.query.trim().is_empty() {
        return Err(invalid_arguments("query must not be empty"));
    }
    let offset = parse_offset(input.offset)?;
    Ok(MessageSearchRequest {
        query: input.query,
        user_id: input.user_id,
        date_from: parse_timestamp(input.date_from, DateBoundary::Start)?,
        date_to: parse_timestamp(input.date_to, DateBoundary::End)?,
        reply_to_message_id: input.reply_to_message_id,
        is_automatic_forward: input.is_automatic_forward,
        has_reply: input.has_reply,
        has_links: input.has_links,
        has_media: input.has_media,
        has_photo: input.has_photo,
        has_video: input.has_video,
        has_document: input.has_document,
        has_audio: input.has_audio,
        has_voice: input.has_voice,
        has_sticker: input.has_sticker,
        has_animation: input.has_animation,
        match_mode: input.match_mode.unwrap_or(MatchMode::Hybrid).into(),
        sort: input.sort.unwrap_or(Sort::Relevance).into(),
        limit: input.limit.unwrap_or(DEFAULT_SEARCH_LIMIT),
        offset,
        include_forwards: input.include_forwards,
    })
}

fn parse_offset(value: Option<i64>) -> Result<i64, rmcp::ErrorData> {
    let offset = value.unwrap_or(0);
    if !(0..=MAX_SEARCH_OFFSET).contains(&offset) {
        return Err(invalid_arguments("offset must be between 0 and 10000"));
    }
    Ok(offset)
}

pub async fn search_messages(
    api: &ChatReadApi,
    input: SearchMessagesInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let page = api
        .search_messages(&search_request(input)?)
        .await
        .map_err(|_| read_error("chat search failed"))?;
    serde_json::to_value(page).map_err(|_| read_error("cannot encode chat result"))
}

pub async fn count_messages(
    api: &ChatReadApi,
    input: CountMessagesInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let request = MessageSearchRequest {
        query: input.query.unwrap_or_default(),
        user_id: input.user_id,
        date_from: parse_timestamp(input.date_from, DateBoundary::Start)?,
        date_to: parse_timestamp(input.date_to, DateBoundary::End)?,
        reply_to_message_id: input.reply_to_message_id,
        is_automatic_forward: input.is_automatic_forward,
        has_reply: input.has_reply,
        has_links: input.has_links,
        has_media: input.has_media,
        has_photo: input.has_photo,
        has_video: input.has_video,
        has_document: input.has_document,
        has_audio: input.has_audio,
        has_voice: input.has_voice,
        has_sticker: input.has_sticker,
        has_animation: input.has_animation,
        match_mode: input.match_mode.unwrap_or(MatchMode::Hybrid).into(),
        sort: MessageSort::Relevance,
        limit: 1,
        offset: 0,
        include_forwards: input.include_forwards,
    };
    let count = api
        .count_messages(&request)
        .await
        .map_err(|_| read_error("chat message count failed"))?;
    Ok(serde_json::json!({"count": count}))
}

pub async fn search_messages_batch(
    api: &ChatReadApi,
    input: SearchMessagesBatchInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let queries = normalize_batch_queries(input.queries)?;
    let date_from = parse_timestamp(input.date_from, DateBoundary::Start)?;
    let date_to = parse_timestamp(input.date_to, DateBoundary::End)?;
    let limit = input
        .limit_per_query
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_BATCH_LIMIT);
    let offset = parse_offset(input.offset)?;
    let mut results = Vec::new();
    for query in queries {
        let messages = api
            .search_messages(&MessageSearchRequest {
                query: query.clone(),
                user_id: input.user_id,
                date_from,
                date_to,
                reply_to_message_id: None,
                is_automatic_forward: input.is_automatic_forward,
                has_reply: input.has_reply,
                has_links: input.has_links,
                has_media: input.has_media,
                has_photo: input.has_photo,
                has_video: input.has_video,
                has_document: input.has_document,
                has_audio: input.has_audio,
                has_voice: input.has_voice,
                has_sticker: input.has_sticker,
                has_animation: input.has_animation,
                match_mode: input.match_mode.clone().unwrap_or(MatchMode::Hybrid).into(),
                sort: input.sort.clone().unwrap_or(Sort::Relevance).into(),
                limit,
                offset,
                include_forwards: input.include_forwards,
            })
            .await
            .map_err(|_| read_error("chat batch search failed"))?;
        results.push(BatchSearchResult {
            query,
            total_count: messages.total_count,
            has_more: messages.has_more,
            next_offset: messages.next_offset,
            scan_limit_reached: messages.scan_limit_reached,
            messages: serde_json::to_value(messages.messages)
                .map_err(|_| read_error("cannot encode chat result"))?,
        });
    }
    Ok(serde_json::json!({"results": results}))
}

fn normalize_batch_queries(queries: Vec<String>) -> Result<Vec<String>, rmcp::ErrorData> {
    if queries.is_empty() {
        return Err(invalid_arguments("queries must not be empty"));
    }
    if queries.len() > MAX_BATCH_QUERIES {
        return Err(invalid_arguments("queries must contain at most six items"));
    }

    let mut normalized = Vec::with_capacity(queries.len());
    for query in queries {
        let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
        if query.is_empty() {
            return Err(invalid_arguments("queries must not contain empty items"));
        }
        if normalized
            .iter()
            .any(|existing: &String| existing.to_lowercase() == query.to_lowercase())
        {
            return Err(invalid_arguments("queries must not contain duplicates"));
        }
        normalized.push(query);
    }
    Ok(normalized)
}

pub async fn recent_messages(
    api: &ChatReadApi,
    input: RecentMessagesInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let messages = api
        .recent_messages(&RecentMessagesRequest {
            user_id: input.user_id,
            date_from: parse_timestamp(input.date_from, DateBoundary::Start)?,
            date_to: parse_timestamp(input.date_to, DateBoundary::End)?,
            is_automatic_forward: input.is_automatic_forward,
            has_reply: input.has_reply,
            has_links: input.has_links,
            has_media: input.has_media,
            has_photo: input.has_photo,
            has_video: input.has_video,
            has_document: input.has_document,
            has_audio: input.has_audio,
            has_voice: input.has_voice,
            has_sticker: input.has_sticker,
            has_animation: input.has_animation,
            limit: input.limit.unwrap_or(DEFAULT_RECENT_LIMIT),
            sort: input.sort.unwrap_or(Sort::Newest).into(),
            include_forwards: input.include_forwards,
        })
        .await
        .map_err(|_| read_error("recent message lookup failed"))?;
    messages_output("messages", messages)
}

pub async fn get_message(
    api: &ChatReadApi,
    input: MessageIdInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let messages = context_messages(api, input.message_id, 0, 0).await?;
    let message = messages
        .as_array()
        .and_then(|messages| messages.first())
        .cloned();
    Ok(serde_json::json!({"found": message.is_some(), "message": message}))
}

pub async fn message_context(
    api: &ChatReadApi,
    input: MessageContextInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let context = context_messages(
        api,
        input.message_id,
        input.before.unwrap_or(DEFAULT_CONTEXT_WINDOW),
        input.after.unwrap_or(DEFAULT_CONTEXT_WINDOW),
    )
    .await?;
    Ok(serde_json::json!({"context": context}))
}

async fn context_messages(
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
    input: MessageIdInput,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let messages = api
        .reply_thread(input.message_id)
        .await
        .map_err(|_| read_error("reply thread lookup failed"))?;
    messages_output("thread", messages)
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
    messages_output("interactions", interactions)
}

fn messages_output(
    field: &'static str,
    values: impl serde::Serialize,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let values =
        serde_json::to_value(values).map_err(|_| read_error("cannot encode chat result"))?;
    Ok(serde_json::json!({field: values}))
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
    fn collection_results_have_an_object_root() {
        let result =
            messages_output("messages", vec![serde_json::json!({"message_id": 1})]).unwrap();
        assert_eq!(result, serde_json::json!({"messages": [{"message_id": 1}]}));
        assert!(result.is_object());
    }

    #[test]
    fn message_id_input_rejects_context_window_fields() {
        assert!(
            serde_json::from_value::<MessageIdInput>(serde_json::json!({
                "message_id": 1,
                "before": 3,
            }))
            .is_err()
        );
    }

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
    fn batch_search_rejects_more_than_six_queries_before_database_access() {
        let queries = (0..=MAX_BATCH_QUERIES)
            .map(|index| format!("query-{index}"))
            .collect::<Vec<_>>();
        let error = normalize_batch_queries(queries).unwrap_err();
        assert_eq!(error.message, "queries must contain at most six items");
    }

    #[test]
    fn batch_search_normalizes_and_rejects_empty_or_duplicate_queries() {
        assert_eq!(
            normalize_batch_queries(vec!["  один   запрос ".into(), "другой".into()]).unwrap(),
            vec!["один запрос", "другой"]
        );
        assert_eq!(
            normalize_batch_queries(vec![" ".into()])
                .unwrap_err()
                .message,
            "queries must not contain empty items"
        );
        assert_eq!(
            normalize_batch_queries(vec!["один запрос".into(), " Один   Запрос ".into()])
                .unwrap_err()
                .message,
            "queries must not contain duplicates"
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
            is_automatic_forward: None,
            has_reply: None,
            has_links: None,
            has_media: None,
            has_photo: None,
            has_video: None,
            has_document: None,
            has_audio: None,
            has_voice: None,
            has_sticker: None,
            has_animation: None,
            match_mode: None,
            sort: None,
            limit: None,
            offset: None,
            include_forwards: false,
        })
        .unwrap_err();
        assert_eq!(error.message, "query must not be empty");
    }

    #[test]
    fn count_input_allows_filter_only_requests() {
        let input = serde_json::from_value::<CountMessagesInput>(serde_json::json!({
            "user_id": 42,
        }))
        .expect("count_messages must allow structural filters without a query");
        assert!(input.query.is_none());
    }

    #[test]
    fn date_only_filters_cover_the_requested_local_day_in_utc() {
        let start = parse_timestamp_value("2026-03-25", DateBoundary::Start).unwrap();
        let end = parse_timestamp_value("2026-03-25", DateBoundary::End).unwrap();
        assert_eq!(start.to_rfc3339(), "2026-03-25T00:00:00+00:00");
        assert_eq!(end.to_rfc3339(), "2026-03-25T23:59:59.999999+00:00");
        assert_eq!(
            parse_timestamp_value("25.03.2026", DateBoundary::Start)
                .unwrap_err()
                .message,
            "timestamps must be RFC 3339 or YYYY-MM-DD"
        );
    }

    #[test]
    fn default_search_mode_is_hybrid() {
        let request = search_request(SearchMessagesInput {
            query: "броня".into(),
            user_id: None,
            date_from: None,
            date_to: None,
            reply_to_message_id: None,
            is_automatic_forward: None,
            has_reply: None,
            has_links: None,
            has_media: None,
            has_photo: None,
            has_video: None,
            has_document: None,
            has_audio: None,
            has_voice: None,
            has_sticker: None,
            has_animation: None,
            match_mode: None,
            sort: None,
            limit: None,
            offset: None,
            include_forwards: false,
        })
        .unwrap();
        assert_eq!(request.match_mode, MessageMatch::Hybrid);
        assert_eq!(request.offset, 0);
        assert!(!request.include_forwards);
    }

    #[test]
    fn search_offset_is_bounded() {
        let error = parse_offset(Some(-1)).unwrap_err();
        assert_eq!(error.message, "offset must be between 0 and 10000");
        assert!(parse_offset(Some(MAX_SEARCH_OFFSET)).is_ok());
        assert!(parse_offset(Some(MAX_SEARCH_OFFSET + 1)).is_err());
    }
}
