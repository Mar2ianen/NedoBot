//! Typed manifest catalog tools.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::features::chat_read_api::ChatReadApi;

use super::invalid_arguments;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTablesInput {}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTablesOutput {
    pub tables: Vec<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeTableInput {
    pub table: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeTableOutput {
    #[serde(flatten)]
    pub description: Value,
}

pub fn list_tables(api: &ChatReadApi) -> ListTablesOutput {
    ListTablesOutput {
        tables: api.list_public_tables(),
    }
}

pub fn describe_table(
    api: &ChatReadApi,
    input: DescribeTableInput,
) -> Result<DescribeTableOutput, rmcp::ErrorData> {
    let description = api
        .describe_public_table(&input.table)
        .ok_or_else(|| invalid_arguments("unknown table"))?;
    Ok(DescribeTableOutput { description })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_rejects_unknown_fields() {
        assert!(
            serde_json::from_value::<DescribeTableInput>(serde_json::json!({
                "table": "telegram_messages",
                "unexpected": true,
            }))
            .is_err()
        );
    }
}
