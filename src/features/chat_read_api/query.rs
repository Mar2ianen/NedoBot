//! Safe generic queries over manifest-reviewed `mcp_public` views.

use anyhow::{bail, ensure};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};

use super::catalog::{CatalogTable, PublicCatalog};

pub const DEFAULT_LIMIT: i64 = 50;
pub const MAX_LIMIT: i64 = 200;
const MAX_FILTERS: usize = 12;
const MAX_COLUMNS: usize = 40;
const MAX_GROUPS: usize = 3;

#[derive(Clone, Debug)]
pub struct SelectRequest {
    pub table: String,
    pub columns: Vec<String>,
    pub filters: Vec<Filter>,
    pub order_by: Vec<OrderBy>,
    pub limit: Option<i64>,
    pub offset: i64,
}

#[derive(Clone, Debug)]
pub struct Filter {
    pub column: String,
    pub op: FilterOp,
    pub value: Option<Value>,
    pub values: Vec<Value>,
    pub case_sensitive: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterOp {
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

#[derive(Clone, Debug)]
pub struct OrderBy {
    pub column: String,
    pub direction: OrderDirection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderDirection {
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Aggregate {
    Count,
    CountDistinct,
    Min,
    Max,
    Sum,
    Avg,
}

#[derive(Clone, Debug)]
pub struct AggregateRequest {
    pub table: String,
    pub operation: Aggregate,
    pub column: Option<String>,
    pub group_by: Vec<String>,
    pub filters: Vec<Filter>,
}

#[derive(Clone, Debug)]
pub struct Page {
    pub rows: Vec<Value>,
    pub next_offset: Option<i64>,
    pub has_more: bool,
}

pub async fn select(
    pool: &PgPool,
    catalog: &PublicCatalog,
    request: SelectRequest,
) -> anyhow::Result<Page> {
    ensure!(
        request.filters.len() <= MAX_FILTERS && request.columns.len() <= MAX_COLUMNS,
        "too many filters or columns"
    );
    let definition = table(catalog, &request.table)?;
    let limit = request.limit.unwrap_or(DEFAULT_LIMIT);
    ensure!(
        (1..=MAX_LIMIT).contains(&limit),
        "limit must be between 1 and {MAX_LIMIT}"
    );
    ensure!(request.offset >= 0, "offset must not be negative");
    let columns = if request.columns.is_empty() {
        definition.columns.keys().cloned().collect()
    } else {
        request.columns
    };
    for column_name in &columns {
        column(definition, column_name)?;
    }

    let selections = columns
        .iter()
        .map(|column_name| {
            let field = column(definition, column_name)?;
            let quoted = quote(column_name);
            Ok(if field.pg_type == "text" {
                format!("left({quoted}, 20000) as {quoted}")
            } else {
                quoted
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?
        .join(", ");
    let mut sql = format!("select {selections} from mcp_public.{}", request.table);
    let mut binds = Vec::new();
    append_filters(&mut sql, definition, &request.filters, &mut binds)?;
    append_order(&mut sql, definition, &request.order_by)?;
    sql.push_str(&format!(" limit {} offset {}", limit + 1, request.offset));
    let mut rows = json_rows(pool, &sql, &binds).await?;
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    Ok(Page {
        rows: rows.into_iter().map(sanitize).collect(),
        next_offset: has_more.then_some(request.offset + limit),
        has_more,
    })
}

pub async fn count(
    pool: &PgPool,
    catalog: &PublicCatalog,
    table_name: String,
    filters: Vec<Filter>,
) -> anyhow::Result<i64> {
    ensure!(filters.len() <= MAX_FILTERS, "too many filters");
    let definition = table(catalog, &table_name)?;
    let mut sql = format!("select count(*)::bigint as count from mcp_public.{table_name}");
    let mut binds = Vec::new();
    append_filters(&mut sql, definition, &filters, &mut binds)?;
    let row = bind_all(sqlx::query(&sql), &binds).fetch_one(pool).await?;
    row.try_get("count").map_err(Into::into)
}

pub async fn aggregate(
    pool: &PgPool,
    catalog: &PublicCatalog,
    request: AggregateRequest,
) -> anyhow::Result<Vec<Value>> {
    ensure!(
        request.filters.len() <= MAX_FILTERS && request.group_by.len() <= MAX_GROUPS,
        "too many filters or grouping columns"
    );
    let definition = table(catalog, &request.table)?;
    let expression = match request.operation {
        Aggregate::Count => "count(*)".to_owned(),
        Aggregate::CountDistinct
        | Aggregate::Min
        | Aggregate::Max
        | Aggregate::Sum
        | Aggregate::Avg => {
            let column_name = request
                .column
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("aggregate column is required"))?;
            column(definition, column_name)?;
            let operation = match request.operation {
                Aggregate::CountDistinct => "count(distinct",
                Aggregate::Min => "min",
                Aggregate::Max => "max",
                Aggregate::Sum => "sum",
                Aggregate::Avg => "avg",
                Aggregate::Count => unreachable!(),
            };
            if request.operation == Aggregate::CountDistinct {
                format!("{operation} {})", quote(column_name))
            } else {
                format!("{operation}({})", quote(column_name))
            }
        }
    };
    for group in &request.group_by {
        column(definition, group)?;
    }
    let groups = request
        .group_by
        .iter()
        .map(|value| quote(value))
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = if groups.is_empty() {
        format!(
            "select {expression} as value from mcp_public.{}",
            request.table
        )
    } else {
        format!(
            "select {groups}, {expression} as value from mcp_public.{}",
            request.table
        )
    };
    let mut binds = Vec::new();
    append_filters(&mut sql, definition, &request.filters, &mut binds)?;
    if !groups.is_empty() {
        sql.push_str(&format!(" group by {groups} limit 500"));
    }
    Ok(json_rows(pool, &sql, &binds)
        .await?
        .into_iter()
        .map(sanitize)
        .collect())
}

fn table<'a>(catalog: &'a PublicCatalog, name: &str) -> anyhow::Result<&'a CatalogTable> {
    catalog
        .tables
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("unknown table"))
}
fn column<'a>(
    table: &'a CatalogTable,
    name: &str,
) -> anyhow::Result<&'a super::catalog::CatalogColumn> {
    table
        .columns
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("unknown column"))
}
fn quote(value: &str) -> String {
    format!("\"{value}\"")
}

fn append_order(
    sql: &mut String,
    definition: &CatalogTable,
    order: &[OrderBy],
) -> anyhow::Result<()> {
    let values = if order.is_empty() {
        definition
            .primary_key
            .iter()
            .map(|name| Ok(quote(name)))
            .collect::<anyhow::Result<Vec<_>>>()?
    } else {
        order
            .iter()
            .map(|item| {
                column(definition, &item.column)?;
                Ok(format!(
                    "{} {}",
                    quote(&item.column),
                    match item.direction {
                        OrderDirection::Asc => "asc",
                        OrderDirection::Desc => "desc",
                    }
                ))
            })
            .collect::<anyhow::Result<Vec<_>>>()?
    };
    sql.push_str(" order by ");
    sql.push_str(&values.join(", "));
    Ok(())
}

fn append_filters(
    sql: &mut String,
    definition: &CatalogTable,
    filters: &[Filter],
    binds: &mut Vec<String>,
) -> anyhow::Result<()> {
    if filters.is_empty() {
        return Ok(());
    }
    let mut parts = Vec::new();
    for filter in filters {
        let field = column(definition, &filter.column)?;
        let name = quote(&filter.column);
        let cast = safe_type(&field.pg_type)?;
        let single = |value: &Option<Value>, binds: &mut Vec<String>| -> anyhow::Result<String> {
            binds.push(value_text(
                value
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("filter value is required"))?,
            )?);
            Ok(format!("${}::{cast}", binds.len()))
        };
        let part = match filter.op {
            FilterOp::Eq => format!("{name} = {}", single(&filter.value, binds)?),
            FilterOp::Ne => format!("{name} <> {}", single(&filter.value, binds)?),
            FilterOp::Lt => format!("{name} < {}", single(&filter.value, binds)?),
            FilterOp::Lte => format!("{name} <= {}", single(&filter.value, binds)?),
            FilterOp::Gt => format!("{name} > {}", single(&filter.value, binds)?),
            FilterOp::Gte => format!("{name} >= {}", single(&filter.value, binds)?),
            FilterOp::IsNull => format!("{name} is null"),
            FilterOp::IsNotNull => format!("{name} is not null"),
            FilterOp::Contains | FilterOp::StartsWith | FilterOp::EndsWith => {
                let value = value_text(
                    filter
                        .value
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("filter value is required"))?,
                )?;
                let pattern = match filter.op {
                    FilterOp::Contains => format!("%{value}%"),
                    FilterOp::StartsWith => format!("{value}%"),
                    FilterOp::EndsWith => format!("%{value}"),
                    _ => unreachable!(),
                };
                binds.push(pattern);
                format!(
                    "{name}::text {} ${}",
                    if filter.case_sensitive {
                        "like"
                    } else {
                        "ilike"
                    },
                    binds.len()
                )
            }
            FilterOp::WholeWord => {
                let value = value_text(
                    filter
                        .value
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("filter value is required"))?,
                )?;
                binds.push(format!(r"\m{}\M", regex_literal(&value)));
                format!(
                    "{name}::text {} ${}",
                    if filter.case_sensitive { "~" } else { "~*" },
                    binds.len()
                )
            }
            FilterOp::Between => {
                ensure!(filter.values.len() == 2, "between requires two values");
                binds.push(value_text(&filter.values[0])?);
                let first = binds.len();
                binds.push(value_text(&filter.values[1])?);
                format!(
                    "{name} between ${first}::{cast} and ${}::{cast}",
                    binds.len()
                )
            }
            FilterOp::In | FilterOp::NotIn => {
                ensure!(
                    !filter.values.is_empty() && filter.values.len() <= 100,
                    "in requires 1 to 100 values"
                );
                let items = filter
                    .values
                    .iter()
                    .map(|value| {
                        binds.push(value_text(value)?);
                        Ok(format!("${}::{cast}", binds.len()))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                format!(
                    "{name} {} in ({})",
                    if filter.op == FilterOp::NotIn {
                        "not"
                    } else {
                        ""
                    },
                    items.join(", ")
                )
            }
        };
        parts.push(part);
    }
    sql.push_str(" where ");
    sql.push_str(&parts.join(" and "));
    Ok(())
}

fn safe_type(value: &str) -> anyhow::Result<&str> {
    match value {
        "bigint"
        | "integer"
        | "smallint"
        | "double precision"
        | "boolean"
        | "text"
        | "timestamp with time zone"
        | "jsonb"
        | "text[]"
        | "integer[]" => Ok(value),
        _ => bail!("unsupported manifest column type"),
    }
}
fn value_text(value: &Value) -> anyhow::Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null => bail!("null must use is_null"),
        _ => Ok(serde_json::to_string(value)?),
    }
}
fn regex_literal(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| {
            if matches!(
                ch,
                '\\' | '.'
                    | '^'
                    | '$'
                    | '|'
                    | '?'
                    | '*'
                    | '+'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '-'
            ) {
                vec!['\\', ch]
            } else {
                vec![ch]
            }
        })
        .collect()
}
async fn json_rows(pool: &PgPool, sql: &str, binds: &[String]) -> anyhow::Result<Vec<Value>> {
    let sql = format!("select to_jsonb(result_row) as row from ({sql}) result_row");
    bind_all(sqlx::query(&sql), binds)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| {
            row.try_get::<sqlx::types::Json<Value>, _>("row")
                .map(|value| value.0)
                .map_err(Into::into)
        })
        .collect()
}
fn bind_all<'a>(
    mut query: sqlx::query::Query<'a, sqlx::Postgres, sqlx::postgres::PgArguments>,
    binds: &'a [String],
) -> sqlx::query::Query<'a, sqlx::Postgres, sqlx::postgres::PgArguments> {
    for value in binds {
        query = query.bind(value);
    }
    query
}
fn sanitize(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(sanitize).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    let hidden = [
                        "token",
                        "access_token",
                        "refresh_token",
                        "api_key",
                        "apikey",
                        "secret",
                        "password",
                        "authorization",
                        "cookie",
                        "database_url",
                        "dsn",
                        "private_key",
                        "client_secret",
                        "webhook_secret",
                        "invite_link",
                        "signed_url",
                    ]
                    .iter()
                    .any(|name| key.eq_ignore_ascii_case(name));
                    (
                        key,
                        if hidden {
                            Value::String("<redacted>".into())
                        } else {
                            sanitize(value)
                        },
                    )
                })
                .collect(),
        ),
        other => other,
    }
}

pub fn page_json(page: Page) -> Value {
    json!({"rows": page.rows, "next_cursor": page.next_offset.map(encode_cursor), "has_more": page.has_more})
}
pub fn encode_cursor(offset: i64) -> String {
    base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        offset.to_be_bytes(),
    )
}
pub fn decode_cursor(cursor: Option<&str>) -> anyhow::Result<i64> {
    let Some(cursor) = cursor else { return Ok(0) };
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, cursor)?;
    ensure!(bytes.len() == 8, "invalid cursor");
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes);
    let offset = i64::from_be_bytes(raw);
    ensure!(offset >= 0, "invalid cursor");
    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_round_trip() {
        assert_eq!(decode_cursor(Some(&encode_cursor(42))).unwrap(), 42);
    }
    #[test]
    fn regex_literal_escapes_metacharacters() {
        assert_eq!(regex_literal("a+b"), "a\\+b");
    }
}
