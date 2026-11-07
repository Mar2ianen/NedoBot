//! Reviewed public catalog metadata for the shared read-model.
//!
//! Transport adapters load it and validate the actual `mcp_public` views before
//! constructing `ChatReadApi`. It deliberately owns neither a pool nor a server.

use std::{collections::BTreeMap, fs};

use anyhow::{Context, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};

use super::types::ChatReadScope;

#[derive(Clone, Debug, Deserialize)]
pub struct PublicCatalog {
    pub(crate) version: u32,
    pub(crate) source_schema: String,
    pub(crate) public_schema: String,
    pub(crate) scope: CatalogScope,
    pub(crate) tables: BTreeMap<String, CatalogTable>,
    #[serde(skip)]
    pub(crate) runtime_scope: Option<ChatReadScope>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CatalogScope {
    #[serde(default)]
    pub(crate) instance_id: Option<String>,
    pub(crate) discussion_chat_id: i64,
    #[serde(default)]
    pub(crate) source_channel_id: Option<i64>,
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
    pub(crate) nullable: bool,
}

impl PublicCatalog {
    pub fn load(path: &str, expected_scope: &ChatReadScope) -> anyhow::Result<Self> {
        let raw =
            fs::read_to_string(path).with_context(|| format!("cannot read MCP manifest {path}"))?;
        let mut catalog: Self = toml::from_str(&raw).context("invalid MCP manifest TOML")?;
        if !matches!(catalog.version, 1 | 2)
            || catalog.source_schema != "public"
            || catalog.public_schema != "mcp_public"
            || catalog.tables.is_empty()
        {
            bail!("invalid MCP manifest metadata");
        }
        if catalog.version == 2 && catalog.scope.instance_id.is_none() {
            bail!("MCP manifest version 2 requires scope.instance_id");
        }
        if catalog
            .scope
            .instance_id
            .as_deref()
            .is_some_and(|instance_id| instance_id != expected_scope.instance_id)
        {
            bail!("MCP manifest instance does not match community configuration");
        }
        let manifest_scope = ChatReadScope::new(
            expected_scope.instance_id.clone(),
            catalog.scope.discussion_chat_id,
            catalog.scope.source_channel_id,
        )?;
        if &manifest_scope != expected_scope {
            bail!("MCP manifest scope does not match community configuration");
        }
        catalog.runtime_scope = Some(expected_scope.clone());
        for (table, definition) in &catalog.tables {
            ensure_identifier(table)?;
            if definition.columns.is_empty() || definition.primary_key.is_empty() {
                bail!("manifest table {table} has no columns or primary key");
            }
            for (column, field) in &definition.columns {
                ensure_identifier(column)?;
                safe_pg_type(&field.pg_type)?;
            }
            for key in &definition.primary_key {
                ensure_identifier(key)?;
                if !definition.columns.contains_key(key) {
                    bail!("manifest primary key {table}.{key} is not a reviewed column");
                }
            }
        }
        Ok(catalog)
    }

    pub fn scope(&self) -> ChatReadScope {
        self.runtime_scope
            .clone()
            .expect("catalog scope is set during manifest loading")
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
                    "filter_operators": filter_operators(&column.pg_type),
                })).collect::<Vec<_>>(),
                "max_limit": 200,
            })
        })
    }

    pub async fn validate_views(&self, pool: &PgPool) -> anyhow::Result<()> {
        for (table, expected) in &self.tables {
            let rows = sqlx::query("select column_name, data_type, udt_name, is_nullable from information_schema.columns where table_schema = 'mcp_public' and table_name = $1")
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
                    let nullable: String = row.get("is_nullable");
                    (
                        name,
                        (normalize_pg_type(&data_type, &udt_name), nullable == "YES"),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            if actual.len() != expected.columns.len() {
                bail!("MCP manifest drift: unreviewed column in {table}");
            }
            for (column, expected_column) in &expected.columns {
                if actual.get(column)
                    != Some(&(expected_column.pg_type.clone(), expected_column.nullable))
                {
                    bail!("MCP manifest drift in {table}.{column}");
                }
            }
            let definition: Option<String> = sqlx::query_scalar(
                "select pg_get_viewdef(format('%I.%I', 'mcp_public', $1)::regclass, true)",
            )
            .bind(table)
            .fetch_one(pool)
            .await?;
            let definition = definition.ok_or_else(|| {
                anyhow::anyhow!("required MCP view mcp_public.{table} is missing")
            })?;
            if !definition.contains("mcp_private.instance_scope") {
                bail!("MCP view mcp_public.{table} is not bound to the database instance scope");
            }
        }
        let media_definition: Option<String> = sqlx::query_scalar(
            "select pg_get_viewdef('mcp_private.telegram_media'::regclass, true)",
        )
        .fetch_optional(pool)
        .await
        .context("required internal MCP media view mcp_private.telegram_media is missing")?;
        if !media_definition
            .as_deref()
            .is_some_and(|definition| definition.contains("mcp_private.instance_scope"))
        {
            bail!("internal MCP media view is not bound to the database instance scope");
        }
        Ok(())
    }

    /// The database scope is the enforcement boundary used by every reviewed
    /// view. A config/manifest mismatch prevents the server from advertising tools.
    pub async fn validate_instance_scope(&self, pool: &PgPool) -> anyhow::Result<()> {
        let configured_scope = self
            .runtime_scope
            .as_ref()
            .context("MCP catalog has no runtime scope")?;
        let matches: bool =
            sqlx::query_scalar("select mcp_private.matches_instance_scope($1, $2, $3)")
                .bind(&configured_scope.instance_id)
                .bind(configured_scope.discussion_chat_id)
                .bind(configured_scope.source_channel_id)
                .fetch_one(pool)
                .await
                .context("MCP instance scope is not provisioned in this database")?;
        if !matches {
            bail!("MCP database scope does not match community configuration");
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

fn filter_operators(pg_type: &str) -> &'static [&'static str] {
    const NULL_OPERATORS: &[&str] = &["is_null", "is_not_null"];
    const EQUALITY_OPERATORS: &[&str] = &["eq", "ne", "in", "not_in", "is_null", "is_not_null"];
    const COMPARISON_OPERATORS: &[&str] = &[
        "eq",
        "ne",
        "lt",
        "lte",
        "gt",
        "gte",
        "in",
        "not_in",
        "between",
        "is_null",
        "is_not_null",
    ];
    const TEXT_OPERATORS: &[&str] = &[
        "eq",
        "ne",
        "lt",
        "lte",
        "gt",
        "gte",
        "in",
        "not_in",
        "between",
        "contains",
        "starts_with",
        "ends_with",
        "whole_word",
        "is_null",
        "is_not_null",
    ];
    match pg_type {
        "bigint" | "integer" | "smallint" | "double precision" | "timestamp with time zone" => {
            COMPARISON_OPERATORS
        }
        "text" => TEXT_OPERATORS,
        "boolean" | "jsonb" => EQUALITY_OPERATORS,
        "text[]" | "integer[]" => NULL_OPERATORS,
        _ => &[],
    }
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

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{PublicCatalog, filter_operators};
    use crate::features::chat_read_api::types::ChatReadScope;

    fn manifest(table: &str) -> String {
        format!(
            r#"
version = 1
source_schema = "public"
public_schema = "mcp_public"

[scope]
discussion_chat_id = -1001932061163
source_channel_id = -1001575496091

[tables.{table}]
description = "test"
primary_key = ["id"]

[tables.{table}.columns.id]
pg_type = "bigint"
nullable = false
"#
        )
    }

    #[test]
    fn describe_table_advertises_column_compatible_filters() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(
            manifest("messages")
                .replace("pg_type = \"bigint\"", "pg_type = \"text[]\"")
                .as_bytes(),
        )
        .unwrap();
        let scope = ChatReadScope::new("test", -1001932061163, Some(-1001575496091)).unwrap();
        let catalog = PublicCatalog::load(file.path().to_str().unwrap(), &scope).unwrap();
        let description = catalog.describe_table("messages").unwrap();
        let operators = description["columns"][0]["filter_operators"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>();
        assert_eq!(operators, ["is_null", "is_not_null"]);
        assert!(filter_operators("text").contains(&"whole_word"));
    }

    #[test]
    fn manifest_rejects_empty_primary_key() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(
            manifest("messages")
                .replace("primary_key = [\"id\"]", "primary_key = []")
                .as_bytes(),
        )
        .unwrap();
        let scope = ChatReadScope::new("test", -1001932061163, Some(-1001575496091)).unwrap();
        assert!(PublicCatalog::load(file.path().to_str().unwrap(), &scope).is_err());
    }

    #[test]
    fn manifest_rejects_primary_key_outside_reviewed_columns() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(
            manifest("messages")
                .replace("primary_key = [\"id\"]", "primary_key = [\"missing\"]")
                .as_bytes(),
        )
        .unwrap();
        let scope = ChatReadScope::new("test", -1001932061163, Some(-1001575496091)).unwrap();
        assert!(PublicCatalog::load(file.path().to_str().unwrap(), &scope).is_err());
    }

    #[test]
    fn committed_manifest_reviews_new_user_risk_columns() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/config/mcp_db_manifest.toml");
        let scope = ChatReadScope::new("nedonews", -1001932061163, Some(-1001575496091)).unwrap();
        let catalog = PublicCatalog::load(path, &scope).unwrap();
        let audit = catalog
            .tables
            .get("telegram_new_user_profile_audits")
            .expect("new-user audit view must remain in the public MCP catalog");

        for column in [
            "telegram_user_id_is_recent",
            "telegram_user_id_rank_ratio",
            "risk_signal_breakdown",
            "risk_personal_channel_score",
            "risk_personal_channel_signals",
        ] {
            assert!(
                audit.columns.contains_key(column),
                "MCP manifest must review {column}"
            );
        }

        assert_eq!(
            audit.columns["telegram_user_id_risk_model_version"].pg_type,
            "text"
        );
        assert_eq!(
            audit.columns["telegram_user_id_spam_probability"].pg_type,
            "double precision"
        );
        assert_eq!(
            audit.columns["risk_personal_channel_score"].pg_type,
            "integer"
        );
    }
}
