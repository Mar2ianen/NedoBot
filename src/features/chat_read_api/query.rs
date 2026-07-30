//! Safe generic queries over manifest-reviewed `mcp_public` views.

use anyhow::{bail, ensure};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};

use super::catalog::{CatalogColumn, CatalogTable, PublicCatalog};

pub const DEFAULT_LIMIT: i64 = 50;
pub const MAX_LIMIT: i64 = 200;
const MAX_FILTERS: usize = 12;
pub const MAX_COLUMNS: usize = 40;
pub const MAX_ORDER_COLUMNS: usize = 8;
const MAX_GROUPS: usize = 3;
const MAX_TEXT_CELL_CHARS: usize = 8_192;
const MAX_JSON_CELL_BYTES: usize = 4_096;
const MAX_TEXT_ARRAY_ITEMS: usize = 16;
const MAX_TEXT_ARRAY_ITEM_CHARS: usize = 256;
const MAX_INTEGER_ARRAY_ITEMS: usize = 128;
// RMCP дублирует результат в text и structured content, поэтому оставляем запас для wire envelope.
const MAX_LOGICAL_ROWS_BYTES: usize = 480 * 1024;
const TRUNCATION_MARKER_PREFIX: &str = "__mcp_truncated__";

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
    validate_select(catalog, &request)?;
    let definition = table(catalog, &request.table)?;
    let limit = request.limit.unwrap_or(DEFAULT_LIMIT);
    let columns = effective_columns(definition, &request.columns)?;
    let selections = columns
        .iter()
        .map(|column_name| {
            let field = column(definition, column_name)?;
            selection(column_name, field)
        })
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = format!("select {selections} from mcp_public.{}", request.table);
    let mut binds = Vec::new();
    append_filters(&mut sql, definition, &request.filters, &mut binds)?;
    append_order(&mut sql, definition, &request.order_by)?;
    sql.push_str(&format!(" limit {} offset {}", limit + 1, request.offset));
    let mut rows = json_rows(pool, &sql, &binds).await?;
    let database_has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let (rows, budget_has_more) = sanitize_rows_within_budget(rows)?;
    let has_more = database_has_more || budget_has_more;
    Ok(Page {
        next_offset: has_more.then_some(request.offset + rows.len() as i64),
        rows,
        has_more,
    })
}

pub async fn count(
    pool: &PgPool,
    catalog: &PublicCatalog,
    table_name: String,
    filters: Vec<Filter>,
) -> anyhow::Result<i64> {
    validate_count(catalog, &table_name, &filters)?;
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
    validate_aggregate(catalog, &request)?;
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
        validate_group_column(column(definition, group)?)?;
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
        append_group_order(&mut sql, &groups);
    }
    let (rows, truncated) = sanitize_rows_within_budget(json_rows(pool, &sql, &binds).await?)?;
    ensure!(
        !truncated,
        "public aggregate response exceeds {MAX_LOGICAL_ROWS_BYTES} bytes"
    );
    Ok(rows)
}

pub fn validate_select(catalog: &PublicCatalog, request: &SelectRequest) -> anyhow::Result<()> {
    ensure!(request.filters.len() <= MAX_FILTERS, "too many filters");
    let definition = table(catalog, &request.table)?;
    effective_columns(definition, &request.columns)?;
    let limit = request.limit.unwrap_or(DEFAULT_LIMIT);
    ensure!(
        (1..=MAX_LIMIT).contains(&limit),
        "limit must be between 1 and {MAX_LIMIT}"
    );
    ensure!(request.offset >= 0, "offset must not be negative");

    let mut sql = String::new();
    let mut binds = Vec::new();
    append_filters(&mut sql, definition, &request.filters, &mut binds)?;
    validate_order_columns(&request.order_by)?;
    append_order(&mut sql, definition, &request.order_by)?;
    Ok(())
}

pub fn validate_count(
    catalog: &PublicCatalog,
    table_name: &str,
    filters: &[Filter],
) -> anyhow::Result<()> {
    ensure!(filters.len() <= MAX_FILTERS, "too many filters");
    let definition = table(catalog, table_name)?;
    let mut sql = String::new();
    let mut binds = Vec::new();
    append_filters(&mut sql, definition, filters, &mut binds)
}

pub fn validate_aggregate(
    catalog: &PublicCatalog,
    request: &AggregateRequest,
) -> anyhow::Result<()> {
    ensure!(
        request.filters.len() <= MAX_FILTERS && request.group_by.len() <= MAX_GROUPS,
        "too many filters or grouping columns"
    );
    let definition = table(catalog, &request.table)?;
    match request.operation {
        Aggregate::Count => ensure!(request.column.is_none(), "count does not accept a column"),
        Aggregate::CountDistinct
        | Aggregate::Min
        | Aggregate::Max
        | Aggregate::Sum
        | Aggregate::Avg => {
            let column_name = request
                .column
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("aggregate column is required"))?;
            validate_aggregate_column(request.operation, column(definition, column_name)?)?;
        }
    }
    for group in &request.group_by {
        validate_group_column(column(definition, group)?)?;
    }
    let mut sql = String::new();
    let mut binds = Vec::new();
    append_filters(&mut sql, definition, &request.filters, &mut binds)
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

fn effective_columns(
    definition: &CatalogTable,
    requested: &[String],
) -> anyhow::Result<Vec<String>> {
    let columns = if requested.is_empty() {
        definition.columns.keys().cloned().collect()
    } else {
        requested.to_vec()
    };
    ensure!(
        columns.len() <= MAX_COLUMNS,
        "too many effective columns (maximum is {MAX_COLUMNS})"
    );
    for name in &columns {
        column(definition, name)?;
    }
    Ok(columns)
}

fn selection(name: &str, field: &CatalogColumn) -> anyhow::Result<Vec<String>> {
    let quoted = quote(name);
    let marker = quote(&format!("{TRUNCATION_MARKER_PREFIX}{name}"));
    let (expression, truncated) = match field.pg_type.as_str() {
        "text" => (
            format!(
                "case when char_length({quoted}) > {MAX_TEXT_CELL_CHARS} then left({quoted}, {}) || '…' else {quoted} end",
                MAX_TEXT_CELL_CHARS - 1
            ),
            format!("char_length({quoted}) > {MAX_TEXT_CELL_CHARS}"),
        ),
        "jsonb" => (
            format!(
                "case when octet_length({quoted}::text) > {MAX_JSON_CELL_BYTES} then jsonb_build_object('_truncated', true) else {quoted} end"
            ),
            format!("octet_length({quoted}::text) > {MAX_JSON_CELL_BYTES}"),
        ),
        "text[]" => (
            format!(
                "case when {quoted} is null then null else array(select case when char_length(item) > {MAX_TEXT_ARRAY_ITEM_CHARS} then left(item, {}) || '…' else item end from unnest({quoted}) as item limit {MAX_TEXT_ARRAY_ITEMS}) end",
                MAX_TEXT_ARRAY_ITEM_CHARS - 1
            ),
            format!(
                "cardinality({quoted}) > {MAX_TEXT_ARRAY_ITEMS} or exists (select 1 from unnest({quoted}) as item where char_length(item) > {MAX_TEXT_ARRAY_ITEM_CHARS})"
            ),
        ),
        "integer[]" => (
            format!(
                "case when cardinality({quoted}) > {MAX_INTEGER_ARRAY_ITEMS} then {quoted}[1:{MAX_INTEGER_ARRAY_ITEMS}] else {quoted} end"
            ),
            format!("cardinality({quoted}) > {MAX_INTEGER_ARRAY_ITEMS}"),
        ),
        _ => return Ok(vec![format!("{quoted} as {quoted}")]),
    };
    Ok(vec![
        format!("{expression} as {quoted}"),
        format!("{truncated} as {marker}"),
    ])
}

fn quote(value: &str) -> String {
    format!("\"{value}\"")
}

fn validate_order_columns(order: &[OrderBy]) -> anyhow::Result<()> {
    ensure!(
        order.len() <= MAX_ORDER_COLUMNS,
        "too many order columns (maximum is {MAX_ORDER_COLUMNS})"
    );
    Ok(())
}

fn append_group_order(sql: &mut String, groups: &str) {
    sql.push_str(&format!(
        " group by {groups} order by {groups} asc limit 500"
    ));
}

fn append_order(
    sql: &mut String,
    definition: &CatalogTable,
    order: &[OrderBy],
) -> anyhow::Result<()> {
    let mut values = if order.is_empty() {
        definition
            .primary_key
            .iter()
            .map(|name| quote(name))
            .collect()
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
    if !order.is_empty() {
        for key in &definition.primary_key {
            if !order.iter().any(|item| item.column == *key) {
                values.push(format!("{} asc", quote(key)));
            }
        }
    }
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
        validate_filter(field, filter)?;
        let kind = column_kind(field)?;
        let name = quote(&filter.column);
        let cast = safe_type(&field.pg_type)?;
        let single = |value: &Option<Value>, binds: &mut Vec<String>| -> anyhow::Result<String> {
            binds.push(value_text(
                value
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("filter value is required"))?,
                kind,
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
                    kind,
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
                    kind,
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
                binds.push(value_text(&filter.values[0], kind)?);
                let first = binds.len();
                binds.push(value_text(&filter.values[1], kind)?);
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
                        binds.push(value_text(value, kind)?);
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

#[derive(Clone, Copy, Eq, PartialEq)]
enum ColumnKind {
    BigInteger,
    Integer,
    SmallInteger,
    Double,
    Boolean,
    Text,
    Timestamp,
    Json,
    Array,
}

fn column_kind(field: &CatalogColumn) -> anyhow::Result<ColumnKind> {
    match field.pg_type.as_str() {
        "bigint" => Ok(ColumnKind::BigInteger),
        "integer" => Ok(ColumnKind::Integer),
        "smallint" => Ok(ColumnKind::SmallInteger),
        "double precision" => Ok(ColumnKind::Double),
        "boolean" => Ok(ColumnKind::Boolean),
        "text" => Ok(ColumnKind::Text),
        "timestamp with time zone" => Ok(ColumnKind::Timestamp),
        "jsonb" => Ok(ColumnKind::Json),
        "text[]" | "integer[]" => Ok(ColumnKind::Array),
        _ => bail!("unsupported manifest column type"),
    }
}

fn validate_group_column(field: &CatalogColumn) -> anyhow::Result<()> {
    ensure!(
        matches!(
            column_kind(field)?,
            ColumnKind::BigInteger
                | ColumnKind::Integer
                | ColumnKind::SmallInteger
                | ColumnKind::Double
                | ColumnKind::Boolean
                | ColumnKind::Timestamp
        ),
        "grouping variable-size columns is not supported"
    );
    Ok(())
}

fn validate_aggregate_column(operation: Aggregate, field: &CatalogColumn) -> anyhow::Result<()> {
    let kind = column_kind(field)?;
    let supported = match operation {
        Aggregate::CountDistinct => true,
        Aggregate::Min | Aggregate::Max => matches!(
            kind,
            ColumnKind::BigInteger
                | ColumnKind::Integer
                | ColumnKind::SmallInteger
                | ColumnKind::Double
                | ColumnKind::Text
                | ColumnKind::Timestamp
        ),
        Aggregate::Sum | Aggregate::Avg => matches!(
            kind,
            ColumnKind::BigInteger
                | ColumnKind::Integer
                | ColumnKind::SmallInteger
                | ColumnKind::Double
        ),
        Aggregate::Count => unreachable!(),
    };
    ensure!(supported, "aggregate is not supported for this column type");
    Ok(())
}

fn validate_filter(field: &CatalogColumn, filter: &Filter) -> anyhow::Result<()> {
    let kind = column_kind(field)?;
    let requires_value = matches!(
        filter.op,
        FilterOp::Eq
            | FilterOp::Ne
            | FilterOp::Lt
            | FilterOp::Lte
            | FilterOp::Gt
            | FilterOp::Gte
            | FilterOp::Contains
            | FilterOp::StartsWith
            | FilterOp::EndsWith
            | FilterOp::WholeWord
    );
    if requires_value {
        ensure!(filter.value.is_some(), "filter value is required");
        ensure!(filter.values.is_empty(), "filter does not accept values");
    }
    if matches!(filter.op, FilterOp::In | FilterOp::NotIn) {
        ensure!(filter.value.is_none(), "in filter does not accept value");
        ensure!(
            !filter.values.is_empty() && filter.values.len() <= 100,
            "in requires 1 to 100 values"
        );
    }
    if filter.op == FilterOp::Between {
        ensure!(
            filter.value.is_none(),
            "between filter does not accept value"
        );
        ensure!(filter.values.len() == 2, "between requires two values");
    }
    if matches!(filter.op, FilterOp::IsNull | FilterOp::IsNotNull) {
        ensure!(
            filter.value.is_none() && filter.values.is_empty(),
            "null filter does not accept values"
        );
        return Ok(());
    }
    ensure!(
        kind != ColumnKind::Array,
        "filtering array columns is not supported"
    );
    if matches!(
        filter.op,
        FilterOp::Contains | FilterOp::StartsWith | FilterOp::EndsWith | FilterOp::WholeWord
    ) {
        ensure!(
            kind == ColumnKind::Text,
            "text matching requires a text column"
        );
    }
    if matches!(
        filter.op,
        FilterOp::Lt | FilterOp::Lte | FilterOp::Gt | FilterOp::Gte | FilterOp::Between
    ) {
        ensure!(
            matches!(
                kind,
                ColumnKind::BigInteger
                    | ColumnKind::Integer
                    | ColumnKind::SmallInteger
                    | ColumnKind::Double
                    | ColumnKind::Text
                    | ColumnKind::Timestamp
            ),
            "comparison is not supported for this column type"
        );
    }
    if let Some(value) = &filter.value {
        validate_value(kind, value)?;
    }
    for value in &filter.values {
        validate_value(kind, value)?;
    }
    Ok(())
}

fn validate_value(kind: ColumnKind, value: &Value) -> anyhow::Result<()> {
    ensure!(!value.is_null(), "null must use is_null");
    match kind {
        ColumnKind::BigInteger => ensure!(value.as_i64().is_some(), "expected a bigint value"),
        ColumnKind::Integer => ensure!(
            value
                .as_i64()
                .is_some_and(|value| i32::try_from(value).is_ok()),
            "expected an integer value"
        ),
        ColumnKind::SmallInteger => ensure!(
            value
                .as_i64()
                .is_some_and(|value| i16::try_from(value).is_ok()),
            "expected a smallint value"
        ),
        ColumnKind::Double => ensure!(value.is_number(), "expected a numeric value"),
        ColumnKind::Boolean => ensure!(value.is_boolean(), "expected a boolean value"),
        ColumnKind::Text => ensure!(value.is_string(), "expected a text value"),
        ColumnKind::Timestamp => {
            let value = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("expected an RFC 3339 timestamp"))?;
            sqlx::types::chrono::DateTime::parse_from_rfc3339(value)
                .map_err(|_| anyhow::anyhow!("expected an RFC 3339 timestamp"))?;
        }
        ColumnKind::Json => {}
        ColumnKind::Array => bail!("filtering array columns is not supported"),
    }
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
fn value_text(value: &Value, kind: ColumnKind) -> anyhow::Result<String> {
    if kind == ColumnKind::Json {
        return Ok(serde_json::to_string(value)?);
    }
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null => bail!("null must use is_null"),
        _ => bail!("expected a scalar value"),
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
fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if [
        "token",
        "bottoken",
        "accesstoken",
        "refreshtoken",
        "apikey",
        "secret",
        "password",
        "authorization",
        "cookie",
        "databaseurl",
        "dsn",
        "privatekey",
        "clientsecret",
        "webhooksecret",
        "invitelink",
        "signedurl",
    ]
    .iter()
    .any(|name| normalized == name.as_bytes())
    {
        return true;
    }

    sensitive_key_suffix(key)
}

fn sensitive_key_suffix(key: &str) -> bool {
    let mut previous = None;
    let mut segment = String::new();
    let mut segments = Vec::new();
    let mut chars = key.chars().peekable();
    while let Some(ch) = chars.next() {
        if !ch.is_ascii_alphanumeric() {
            if !segment.is_empty() {
                segments.push(std::mem::take(&mut segment));
            }
            previous = None;
            continue;
        }
        let next_is_lowercase = chars.peek().is_some_and(|next| next.is_ascii_lowercase());
        if ch.is_ascii_uppercase()
            && !segment.is_empty()
            && (previous.is_some_and(|previous: char| previous.is_ascii_lowercase())
                || (previous.is_some_and(|previous: char| previous.is_ascii_uppercase())
                    && next_is_lowercase))
        {
            segments.push(std::mem::take(&mut segment));
        }
        segment.push(ch.to_ascii_lowercase());
        previous = Some(ch);
    }
    if !segment.is_empty() {
        segments.push(segment);
    }
    matches!(
        segments.last().map(String::as_str),
        Some("key" | "token" | "secret" | "password")
    )
}

fn sanitize_rows_within_budget(rows: Vec<Value>) -> anyhow::Result<(Vec<Value>, bool)> {
    let mut bytes = 2;
    let mut sanitized = Vec::with_capacity(rows.len());
    for row in rows {
        let row = sanitize(row);
        let row_bytes = serde_json::to_vec(&row)?.len() + 1;
        if bytes + row_bytes > MAX_LOGICAL_ROWS_BYTES {
            ensure!(
                !sanitized.is_empty(),
                "single public query row exceeds logical rows budget"
            );
            return Ok((sanitized, true));
        }
        bytes += row_bytes;
        sanitized.push(row);
    }
    Ok((sanitized, false))
}

fn sanitize(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(sanitize).collect()),
        Value::Object(values) => {
            let mut sanitized = serde_json::Map::new();
            let mut truncated_fields = Vec::new();
            for (key, value) in values {
                if let Some(field) = key.strip_prefix(TRUNCATION_MARKER_PREFIX) {
                    if value == Value::Bool(true) {
                        truncated_fields.push(Value::String(field.to_owned()));
                    }
                    continue;
                }
                let value = if is_sensitive_key(&key) {
                    Value::String("<redacted>".into())
                } else {
                    sanitize(value)
                };
                sanitized.insert(key, value);
            }
            if !truncated_fields.is_empty() {
                sanitized.insert(
                    "_truncated_fields".to_owned(),
                    Value::Array(truncated_fields),
                );
            }
            Value::Object(sanitized)
        }
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
    use std::collections::BTreeMap;

    use super::*;
    use crate::features::chat_read_api::catalog::{
        CatalogColumn, CatalogScope, CatalogTable, PublicCatalog,
    };

    fn table(primary_key: &[&str]) -> CatalogTable {
        let mut columns = BTreeMap::new();
        for name in ["id", "created_at", "message_id"] {
            columns.insert(
                name.to_string(),
                CatalogColumn {
                    pg_type: "bigint".to_string(),
                    nullable: false,
                },
            );
        }
        CatalogTable {
            description: "test".to_string(),
            primary_key: primary_key.iter().map(ToString::to_string).collect(),
            approximate_rows: None,
            columns,
        }
    }

    fn catalog(columns: &[(&str, &str)]) -> PublicCatalog {
        let mut definition = table(&["id"]);
        for (name, pg_type) in columns {
            definition.columns.insert(
                (*name).to_string(),
                CatalogColumn {
                    pg_type: (*pg_type).to_string(),
                    nullable: true,
                },
            );
        }
        PublicCatalog {
            version: 1,
            source_schema: "public".to_string(),
            public_schema: "mcp_public".to_string(),
            scope: CatalogScope {
                discussion_chat_id: 1,
                source_channel_id: 2,
            },
            tables: BTreeMap::from([("test".to_string(), definition)]),
        }
    }

    fn filter(column: &str, op: FilterOp, value: Option<Value>, values: Vec<Value>) -> Filter {
        Filter {
            column: column.to_string(),
            op,
            value,
            values,
            case_sensitive: false,
        }
    }

    #[test]
    fn cursor_round_trip() {
        assert_eq!(decode_cursor(Some(&encode_cursor(42))).unwrap(), 42);
    }

    #[test]
    fn regex_literal_escapes_metacharacters() {
        assert_eq!(regex_literal("a+b"), "a\\+b");
    }

    #[test]
    fn user_order_appends_missing_primary_key_columns() {
        let mut sql = String::new();
        append_order(
            &mut sql,
            &table(&["id", "message_id"]),
            &[OrderBy {
                column: "created_at".to_string(),
                direction: OrderDirection::Desc,
            }],
        )
        .unwrap();
        assert_eq!(
            sql,
            " order by \"created_at\" desc, \"id\" asc, \"message_id\" asc"
        );
    }

    #[test]
    fn user_order_does_not_repeat_primary_key_columns() {
        let mut sql = String::new();
        append_order(
            &mut sql,
            &table(&["id", "message_id"]),
            &[OrderBy {
                column: "id".to_string(),
                direction: OrderDirection::Desc,
            }],
        )
        .unwrap();
        assert_eq!(sql, " order by \"id\" desc, \"message_id\" asc");
    }

    #[test]
    fn caps_order_columns() {
        let order = (0..=MAX_ORDER_COLUMNS)
            .map(|_| OrderBy {
                column: "id".to_string(),
                direction: OrderDirection::Asc,
            })
            .collect::<Vec<_>>();
        assert!(validate_order_columns(&order).is_err());
    }

    #[test]
    fn validation_caps_default_columns_for_wide_tables() {
        let mut catalog = catalog(&[]);
        let definition = catalog.tables.get_mut("test").unwrap();
        for index in 0..=MAX_COLUMNS {
            definition.columns.insert(
                format!("column_{index}"),
                CatalogColumn {
                    pg_type: "text".to_string(),
                    nullable: true,
                },
            );
        }
        let request = SelectRequest {
            table: "test".to_string(),
            columns: vec![],
            filters: vec![],
            order_by: vec![],
            limit: None,
            offset: 0,
        };
        assert!(validate_select(&catalog, &request).is_err());
    }

    #[test]
    fn response_budget_returns_a_partial_page() {
        let rows = vec![
            json!({"text": "a".repeat(MAX_LOGICAL_ROWS_BYTES / 2 + 100)}),
            json!({"text": "b".repeat(MAX_LOGICAL_ROWS_BYTES / 2 + 100)}),
        ];
        let (rows, has_more) = sanitize_rows_within_budget(rows).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(has_more);
    }

    #[test]
    fn response_budget_rejects_a_single_oversized_row() {
        let rows = vec![json!({"text": "a".repeat(MAX_LOGICAL_ROWS_BYTES)})];
        assert!(sanitize_rows_within_budget(rows).is_err());
    }

    #[test]
    fn text_array_selection_marks_item_and_cardinality_truncation() {
        let field = CatalogColumn {
            pg_type: "text[]".to_string(),
            nullable: true,
        };
        let sql = selection("entities", &field).unwrap().join(", ");
        assert!(sql.contains("char_length(item) > 256"));
        assert!(sql.contains("|| '…'"));
        assert!(sql.contains("cardinality(\"entities\") > 16"));
        assert!(sql.contains("exists (select 1 from unnest(\"entities\")"));
    }

    #[test]
    fn sanitizer_exposes_truncated_fields_without_internal_markers() {
        assert_eq!(
            sanitize(json!({
                "text": "preview…",
                "__mcp_truncated__text": true,
                "__mcp_truncated__json": false,
            })),
            json!({"text": "preview…", "_truncated_fields": ["text"]})
        );
    }

    #[test]
    fn sanitizer_normalizes_sensitive_key_separators_and_camel_case() {
        for key in [
            "api-key",
            "bot_token",
            "clientSecret",
            "private-key",
            "openai_api_key",
            "anthropicApiKey",
            "openAIKey",
        ] {
            assert!(is_sensitive_key(key), "{key} should be redacted");
        }
        for key in ["token_count", "secretary", "client_secret_name", "monkey"] {
            assert!(!is_sensitive_key(key), "{key} should remain visible");
        }
        assert_eq!(
            sanitize(json!({"openai_api_key": "secret", "model": "gpt"})),
            json!({"openai_api_key": "<redacted>", "model": "gpt"})
        );
    }

    #[test]
    fn validation_rejects_mismatched_filter_values_before_database_access() {
        let catalog = catalog(&[
            ("enabled", "boolean"),
            ("created", "timestamp with time zone"),
        ]);
        let request = SelectRequest {
            table: "test".to_string(),
            columns: vec![],
            filters: vec![filter("enabled", FilterOp::Eq, Some(json!("true")), vec![])],
            order_by: vec![],
            limit: None,
            offset: 0,
        };
        assert!(validate_select(&catalog, &request).is_err());

        let request = SelectRequest {
            filters: vec![filter(
                "created",
                FilterOp::Gte,
                Some(json!("not-a-date")),
                vec![],
            )],
            ..request
        };
        assert!(validate_select(&catalog, &request).is_err());
    }

    #[test]
    fn validation_rejects_out_of_range_integer_filters_before_database_access() {
        let catalog = catalog(&[("sequence", "integer")]);
        for value in [json!(2_147_483_648_i64), json!(-2_147_483_649_i64)] {
            let request = SelectRequest {
                table: "test".to_string(),
                columns: vec![],
                filters: vec![filter("sequence", FilterOp::Eq, Some(value), vec![])],
                order_by: vec![],
                limit: None,
                offset: 0,
            };
            assert!(validate_select(&catalog, &request).is_err());
        }
    }

    #[test]
    fn jsonb_filter_values_are_serialized_as_json() {
        assert_eq!(
            value_text(&json!("value"), ColumnKind::Json).unwrap(),
            "\"value\""
        );
        assert_eq!(
            value_text(&json!({"key": "value"}), ColumnKind::Json).unwrap(),
            "{\"key\":\"value\"}"
        );
    }

    #[test]
    fn validation_rejects_array_filters_except_null_checks() {
        let catalog = catalog(&[("tags", "text[]")]);
        let request = SelectRequest {
            table: "test".to_string(),
            columns: vec![],
            filters: vec![filter("tags", FilterOp::Eq, Some(json!(["rust"])), vec![])],
            order_by: vec![],
            limit: None,
            offset: 0,
        };
        assert!(validate_select(&catalog, &request).is_err());

        let request = SelectRequest {
            filters: vec![filter("tags", FilterOp::IsNull, None, vec![])],
            ..request
        };
        assert!(validate_select(&catalog, &request).is_ok());
    }

    #[test]
    fn aggregate_validation_rejects_sum_of_text_before_database_access() {
        let catalog = catalog(&[("body", "text")]);
        let request = AggregateRequest {
            table: "test".to_string(),
            operation: Aggregate::Sum,
            column: Some("body".to_string()),
            group_by: vec![],
            filters: vec![],
        };
        assert!(validate_aggregate(&catalog, &request).is_err());
    }

    #[test]
    fn grouped_aggregate_sql_orders_by_group_columns() {
        let mut sql = "select count(*)".to_string();
        append_group_order(&mut sql, "\"created_at\", \"message_id\"");
        assert_eq!(
            sql,
            "select count(*) group by \"created_at\", \"message_id\" order by \"created_at\", \"message_id\" asc limit 500"
        );
    }
}
