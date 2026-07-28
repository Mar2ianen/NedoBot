//! Strict typed contracts for generic queries over reviewed public views.

use std::collections::BTreeMap;

use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{invalid_arguments, read_error};
use crate::features::chat_read_api::{ChatReadApi, query};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectInput {
    pub table: String,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub filters: Vec<FilterInput>,
    #[serde(default)]
    pub order_by: Vec<OrderByInput>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchRowInput {
    pub table: String,
    pub key: BTreeMap<String, Value>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CountInput {
    pub table: String,
    #[serde(default)]
    pub filters: Vec<FilterInput>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AggregateInput {
    pub table: String,
    pub operation: AggregateOperation,
    pub column: Option<String>,
    #[serde(default)]
    pub group_by: Vec<String>,
    #[serde(default)]
    pub filters: Vec<FilterInput>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchTextInput {
    pub table: String,
    pub column: String,
    pub query: String,
    pub match_mode: Option<SearchMatchMode>,
    pub case_sensitive: Option<bool>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AggregateOperation {
    Count,
    CountDistinct,
    Min,
    Max,
    Sum,
    Avg,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchMatchMode {
    Contains,
    WholeWord,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilterInput {
    pub column: String,
    pub op: FilterOperation,
    pub value: Option<Value>,
    #[serde(default)]
    pub values: Vec<Value>,
    #[serde(default)]
    pub case_sensitive: bool,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterOperation {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    In,
    NotIn,
    IsNull,
    IsNotNull,
    Contains,
    StartsWith,
    EndsWith,
    Between,
    WholeWord,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrderByInput {
    pub column: String,
    pub direction: OrderDirection,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrderDirection {
    Asc,
    Desc,
}

impl From<FilterOperation> for query::FilterOp {
    fn from(value: FilterOperation) -> Self {
        match value {
            FilterOperation::Eq => Self::Eq,
            FilterOperation::Ne => Self::Ne,
            FilterOperation::Lt => Self::Lt,
            FilterOperation::Lte => Self::Lte,
            FilterOperation::Gt => Self::Gt,
            FilterOperation::Gte => Self::Gte,
            FilterOperation::In => Self::In,
            FilterOperation::NotIn => Self::NotIn,
            FilterOperation::IsNull => Self::IsNull,
            FilterOperation::IsNotNull => Self::IsNotNull,
            FilterOperation::Contains => Self::Contains,
            FilterOperation::StartsWith => Self::StartsWith,
            FilterOperation::EndsWith => Self::EndsWith,
            FilterOperation::Between => Self::Between,
            FilterOperation::WholeWord => Self::WholeWord,
        }
    }
}
impl From<OrderDirection> for query::OrderDirection {
    fn from(value: OrderDirection) -> Self {
        match value {
            OrderDirection::Asc => Self::Asc,
            OrderDirection::Desc => Self::Desc,
        }
    }
}
impl From<AggregateOperation> for query::Aggregate {
    fn from(value: AggregateOperation) -> Self {
        match value {
            AggregateOperation::Count => Self::Count,
            AggregateOperation::CountDistinct => Self::CountDistinct,
            AggregateOperation::Min => Self::Min,
            AggregateOperation::Max => Self::Max,
            AggregateOperation::Sum => Self::Sum,
            AggregateOperation::Avg => Self::Avg,
        }
    }
}
fn filters(values: Vec<FilterInput>) -> Vec<query::Filter> {
    values
        .into_iter()
        .map(|value| query::Filter {
            column: value.column,
            op: value.op.into(),
            value: value.value,
            values: value.values,
            case_sensitive: value.case_sensitive,
        })
        .collect()
}

pub async fn select(api: &ChatReadApi, input: SelectInput) -> Result<Value, rmcp::ErrorData> {
    let offset = query::decode_cursor(input.cursor.as_deref())
        .map_err(|_| invalid_arguments("invalid cursor"))?;
    let request = query::SelectRequest {
        table: input.table,
        columns: input.columns,
        filters: filters(input.filters),
        order_by: input
            .order_by
            .into_iter()
            .map(|value| query::OrderBy {
                column: value.column,
                direction: value.direction.into(),
            })
            .collect(),
        limit: input.limit,
        offset,
    };
    query::validate_select(api.catalog(), &request)
        .map_err(|err| invalid_arguments(err.to_string()))?;
    let page = api
        .select_public(request)
        .await
        .map_err(|_| read_error("public select failed"))?;
    Ok(query::page_json(page))
}
pub async fn fetch_row(api: &ChatReadApi, input: FetchRowInput) -> Result<Value, rmcp::ErrorData> {
    let description = api
        .describe_public_table(&input.table)
        .ok_or_else(|| invalid_arguments("unknown table"))?;
    let keys = description["primary_key"]
        .as_array()
        .ok_or_else(|| read_error("invalid catalog"))?;
    if input.key.len() != keys.len()
        || !keys
            .iter()
            .all(|key| key.as_str().is_some_and(|key| input.key.contains_key(key)))
    {
        return Err(invalid_arguments(
            "key must contain exactly the full primary key",
        ));
    }
    let request = query::SelectRequest {
        table: input.table,
        columns: vec![],
        filters: input
            .key
            .into_iter()
            .map(|(column, value)| query::Filter {
                column,
                op: query::FilterOp::Eq,
                value: Some(value),
                values: vec![],
                case_sensitive: false,
            })
            .collect(),
        order_by: vec![],
        limit: Some(1),
        offset: 0,
    };
    query::validate_select(api.catalog(), &request)
        .map_err(|err| invalid_arguments(err.to_string()))?;
    let page = api
        .select_public(request)
        .await
        .map_err(|_| read_error("public row lookup failed"))?;
    Ok(json!({"row": page.rows.into_iter().next()}))
}
pub async fn count(api: &ChatReadApi, input: CountInput) -> Result<Value, rmcp::ErrorData> {
    let filters = filters(input.filters);
    query::validate_count(api.catalog(), &input.table, &filters)
        .map_err(|err| invalid_arguments(err.to_string()))?;
    let count = api
        .count_public(input.table, filters)
        .await
        .map_err(|_| read_error("public count failed"))?;
    Ok(json!({"count": count}))
}
pub async fn aggregate(api: &ChatReadApi, input: AggregateInput) -> Result<Value, rmcp::ErrorData> {
    let request = query::AggregateRequest {
        table: input.table,
        operation: input.operation.into(),
        column: input.column,
        group_by: input.group_by,
        filters: filters(input.filters),
    };
    query::validate_aggregate(api.catalog(), &request)
        .map_err(|err| invalid_arguments(err.to_string()))?;
    let rows = api
        .aggregate_public(request)
        .await
        .map_err(|_| read_error("public aggregate failed"))?;
    Ok(json!({"rows": rows}))
}
pub async fn search_text(
    api: &ChatReadApi,
    input: SearchTextInput,
) -> Result<Value, rmcp::ErrorData> {
    if input.query.trim().is_empty() {
        return Err(invalid_arguments("query must not be empty"));
    }
    select(
        api,
        SelectInput {
            table: input.table,
            columns: vec![],
            filters: vec![FilterInput {
                column: input.column,
                op: match input.match_mode.unwrap_or(SearchMatchMode::Contains) {
                    SearchMatchMode::Contains => FilterOperation::Contains,
                    SearchMatchMode::WholeWord => FilterOperation::WholeWord,
                },
                value: Some(Value::String(input.query)),
                values: vec![],
                case_sensitive: input.case_sensitive.unwrap_or(false),
            }],
            order_by: vec![],
            limit: input.limit,
            cursor: input.cursor,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn select_rejects_unknown_fields() {
        assert!(
            serde_json::from_value::<SelectInput>(
                json!({"table":"telegram_messages","unexpected":true})
            )
            .is_err()
        );
    }
    #[test]
    fn filter_rejects_unknown_fields() {
        assert!(
            serde_json::from_value::<FilterInput>(json!({"column":"text","op":"eq","bad":true}))
                .is_err()
        );
    }
}
