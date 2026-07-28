//! Legacy semantic contracts implemented through `ChatReadApi`.

use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{invalid_arguments, read_error};
use crate::features::chat_read_api::{ChatReadApi, query};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolveUserInput {
    pub query: Option<String>,
    pub telegram_user_id: Option<i64>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserNotesInput {
    pub telegram_user_id: i64,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyInput {}

pub async fn resolve_user(
    api: &ChatReadApi,
    input: ResolveUserInput,
) -> Result<Value, rmcp::ErrorData> {
    if input.telegram_user_id.is_none()
        && input
            .query
            .as_deref()
            .is_none_or(|query| query.trim().is_empty())
    {
        return Err(invalid_arguments("query or telegram_user_id is required"));
    }
    let users = api
        .resolve_users(input.telegram_user_id, input.query.as_deref())
        .await
        .map_err(|_| read_error("user resolution failed"))?;
    Ok(
        json!({"users": users.into_iter().map(|user| json!({"telegram_user_id":user.telegram_user_id,"username":user.username,"display_name":user.display_name,"author_url":public_username_url(user.username.as_deref()),"message_count":user.message_count,"match":user.match_kind,"recommended":user.recommended})).collect::<Vec<_>>() }),
    )
}
pub async fn list_chat_notes(api: &ChatReadApi, _: EmptyInput) -> Result<Value, rmcp::ErrorData> {
    let notes = api
        .chat_notes()
        .await
        .map_err(|_| read_error("chat notes lookup failed"))?;
    Ok(json!({"notes": notes}))
}
pub async fn list_user_notes(
    api: &ChatReadApi,
    input: UserNotesInput,
) -> Result<Value, rmcp::ErrorData> {
    let notes = api
        .user_notes(input.telegram_user_id)
        .await
        .map_err(|_| read_error("user notes lookup failed"))?;
    Ok(json!({"notes": notes}))
}

pub async fn list_view(
    api: &ChatReadApi,
    table: &'static str,
    order_column: &'static str,
    filters: Vec<query::Filter>,
) -> Result<Value, rmcp::ErrorData> {
    let page = api
        .select_public(query::SelectRequest {
            table: table.into(),
            columns: vec![],
            filters,
            order_by: vec![query::OrderBy {
                column: order_column.into(),
                direction: query::OrderDirection::Desc,
            }],
            limit: Some(100),
            offset: 0,
        })
        .await
        .map_err(|_| read_error("public list failed"))?;
    Ok(query::page_json(page))
}

fn public_username_url(username: Option<&str>) -> Option<String> {
    let username = username?.trim();
    ((5..=32).contains(&username.len())
        && username
            .bytes()
            .all(|character| character.is_ascii_alphanumeric() || character == b'_'))
    .then(|| format!("https://t.me/{username}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolve_input_rejects_unknown_fields() {
        assert!(
            serde_json::from_value::<ResolveUserInput>(json!({"query":"alice","extra":true}))
                .is_err()
        );
    }
    #[test]
    fn username_url_requires_telegram_safe_label() {
        assert_eq!(public_username_url(Some("bad name")), None);
    }
}
