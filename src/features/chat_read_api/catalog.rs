//! Reviewed public catalog metadata for the shared read-model.
//!
//! Transport adapters load it and validate the actual `mcp_public` views before
//! constructing `ChatReadApi`. It deliberately owns neither a pool nor a server.

use std::{collections::BTreeMap, fs};

use anyhow::{Context, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};

use super::{policy, types::ChatReadScope};

#[derive(Clone, Debug, Deserialize)]
pub struct PublicCatalog {
    pub(crate) version: u32,
    pub(crate) source_schema: String,
    pub(crate) public_schema: String,
    pub(crate) scope: CatalogScope,
    pub(crate) tables: BTreeMap<String, CatalogTable>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CatalogScope {
    pub(crate) discussion_chat_id: i64,
    pub(crate) source_channel_id: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CatalogTable {
    pub(crate) description: String,
    pub(crate) primary_key: Vec<String>,
    #[serde(default)]
    pub(crate) approximate_rows: Option<i64>,
    pub(crate) columns: BTreeMap<String, CatalogColumn>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CatalogColumn {
    pub(crate) pg_type: String,
    #[serde(default)]
    pub(crate) nullable: bool,
}

impl PublicCatalog {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let raw =
            fs::read_to_string(path).with_context(|| format!("cannot read MCP manifest {path}"))?;
        let catalog: Self = toml::from_str(&raw).context("invalid MCP manifest TOML")?;
        if catalog.version != 1
            || catalog.source_schema != "public"
            || catalog.public_schema != "mcp_public"
            || catalog.scope.discussion_chat_id != policy::DISCUSSION_CHAT_ID
            || catalog.scope.source_channel_id != policy::SOURCE_CHANNEL_ID
            || catalog.tables.is_empty()
        {
            bail!("invalid MCP manifest metadata");
        }
        for (table, definition) in &catalog.tables {
            ensure_identifier(table)?;
            if definition.columns.is_empty() {
                bail!("manifest table {table} has no columns");
            }
            for (column, field) in &definition.columns {
                ensure_identifier(column)?;
                safe_pg_type(&field.pg_type)?;
            }
        }
        Ok(catalog)
    }

    pub fn scope(&self) -> ChatReadScope {
        ChatReadScope {
            discussion_chat_id: self.scope.discussion_chat_id,
            source_channel_id: self.scope.source_channel_id,
        }
    }

    /// Returns the reviewed public catalog without exposing its mutable internals.
    pub fn list_tables(&self) -> Vec<Value> {
        self.tables
            .iter()
            .map(|(name, table)| {
                json!({
                    "name": name,
                    "description": table.description,
                    "primary_key": table.primary_key,
                    "approximate_rows": table.approximate_rows,
                })
            })
            .collect()
    }

    /// Describes exactly one reviewed public view.
    pub fn describe_table(&self, name: &str) -> Option<Value> {
        self.tables.get(name).map(|table| {
            json!({
                "name": name,
                "description": table.description,
                "primary_key": table.primary_key,
                "columns": table.columns.iter().map(|(name, column)| json!({
                    "name": name,
                    "type": column.pg_type,
                    "nullable": column.nullable,
                })).collect::<Vec<_>>(),
                "filter_operators": ["eq", "ne", "lt", "lte", "gt", "gte", "in", "not_in", "is_null", "is_not_null", "contains", "starts_with", "ends_with", "between"],
                "max_limit": 200,
            })
        })
    }

    pub async fn validate_views(&self, pool: &PgPool) -> anyhow::Result<()> {
        for (table, expected) in &self.tables {
            let rows = sqlx::query("select column_name, data_type, udt_name from information_schema.columns where table_schema = 'mcp_public' and table_name = $1")
                .bind(table)
                .fetch_all(pool)
                .await?;
            if rows.is_empty() {
                bail!("required MCP view mcp_public.{table} is missing");
            }
            let actual = rows
                .into_iter()
                .map(|row| {
                    let name: String = row.get("column_name");
                    let data_type: String = row.get("data_type");
                    let udt_name: String = row.get("udt_name");
                    (name, normalize_pg_type(&data_type, &udt_name))
                })
                .collect::<BTreeMap<_, _>>();
            if actual.len() != expected.columns.len() {
                bail!("MCP manifest drift: unreviewed column in {table}");
            }
            for (column, expected_column) in &expected.columns {
                if actual.get(column) != Some(&expected_column.pg_type) {
                    bail!("MCP manifest drift in {table}.{column}");
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn ensure_identifier(value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        bail!("unsafe identifier in MCP manifest");
    }
    Ok(())
}

fn safe_pg_type(value: &str) -> anyhow::Result<()> {
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
        | "integer[]" => Ok(()),
        _ => bail!("unsupported PostgreSQL type in MCP manifest"),
    }
}

fn normalize_pg_type(data_type: &str, udt_name: &str) -> String {
    match data_type {
        "ARRAY" => format!("{}[]", normalize_array_type(udt_name)),
        "USER-DEFINED" => udt_name.into(),
        _ => data_type.into(),
    }
}

fn normalize_array_type(udt_name: &str) -> &str {
    match udt_name.trim_start_matches('_') {
        "int4" => "integer",
        "int8" => "bigint",
        "int2" => "smallint",
        other => other,
    }
}
