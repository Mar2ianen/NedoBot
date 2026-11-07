//! Generates the reviewed MCP manifest from `mcp_public` views.
//! Run only after inspecting the resulting diff before publishing a new view/column.

use std::{collections::BTreeMap, env, fs, path::Path};

use anyhow::{Context, bail};
use serde::Serialize;
use sqlx::{Row, postgres::PgPoolOptions};

use tg_ai_bot_teloxide::{
    config::load_mcp_instance_settings, features::chat_read_api::types::ChatReadScope,
};

#[derive(Serialize)]
struct Manifest {
    version: u32,
    generated_at: String,
    source_schema: String,
    public_schema: String,
    scope: Scope,
    tables: BTreeMap<String, Table>,
}
#[derive(Serialize)]
struct Scope {
    instance_id: String,
    discussion_chat_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_channel_id: Option<i64>,
}
#[derive(Serialize)]
struct Table {
    description: String,
    primary_key: Vec<String>,
    approximate_rows: i64,
    columns: BTreeMap<String, Column>,
}
#[derive(Serialize)]
struct Column {
    pg_type: String,
    nullable: bool,
}

const RETIRED_LEGACY_VIEWS: &[&str] = &[
    "avatar_analysis_jobs",
    "avatar_image_analyses",
    "avatar_profile_assessments",
];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let database_url = env::var("DATABASE_URL")
        .or_else(|_| env::var("ASK_DATABASE_URL"))
        .context("DATABASE_URL or ASK_DATABASE_URL is required")?;
    let profile_path =
        env::var("LLM_PROFILES_PATH").unwrap_or_else(|_| "config/llm_profiles.toml.example".into());
    if profile_path.trim().is_empty() {
        bail!("LLM_PROFILES_PATH must not be empty");
    }
    let configured_scope = load_mcp_instance_settings(&profile_path)?.scope;
    let output = env::args()
        .nth(1)
        .unwrap_or_else(|| "config/mcp_db_manifest.toml".into());
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;
    let scope_row = sqlx::query(
        "select instance_id, discussion_chat_id, source_channel_id from mcp_private.instance_scope where scope_key = true",
    )
    .fetch_all(&pool)
    .await
    .context("MCP instance scope is not provisioned in this database")?;
    if scope_row.len() != 1 {
        bail!("MCP database must contain exactly one instance scope row");
    }
    let row = &scope_row[0];
    let database_scope = ChatReadScope::new(
        row.get::<String, _>("instance_id"),
        row.get("discussion_chat_id"),
        row.get("source_channel_id"),
    )?;
    if database_scope != configured_scope {
        bail!("database scope does not match LLM_PROFILES_PATH community configuration");
    }
    let rows = sqlx::query("select table_name from information_schema.views where table_schema = 'mcp_public' order by table_name").fetch_all(&pool).await?;
    if rows.is_empty() {
        bail!("mcp_public has no views; apply the MCP view migration first");
    }
    let mut tables = BTreeMap::new();
    for row in rows {
        let name: String = row.get("table_name");
        if RETIRED_LEGACY_VIEWS.contains(&name.as_str()) {
            continue;
        }
        let column_rows = sqlx::query("select column_name, data_type, udt_name, is_nullable from information_schema.columns where table_schema = 'mcp_public' and table_name = $1 order by ordinal_position").bind(&name).fetch_all(&pool).await?;
        let columns = column_rows
            .into_iter()
            .map(|row| {
                let data_type: String = row.get("data_type");
                let udt_name: String = row.get("udt_name");
                Ok((
                    row.get("column_name"),
                    Column {
                        pg_type: normalize_type(&data_type, &udt_name)?,
                        nullable: row.get::<String, _>("is_nullable") == "YES",
                    },
                ))
            })
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        let explain_sql = format!("explain (format json) select * from mcp_public.\"{name}\"");
        let explain: sqlx::types::Json<serde_json::Value> =
            sqlx::query_scalar(&explain_sql).fetch_one(&pool).await?;
        let estimate = explain.0[0]["Plan"]["Plan Rows"]
            .as_i64()
            .unwrap_or_default();
        tables.insert(
            name.clone(),
            Table {
                description: description(&name, configured_scope.instance_id()),
                primary_key: primary_key(&name),
                approximate_rows: estimate,
                columns,
            },
        );
    }
    let manifest = Manifest {
        version: 2,
        generated_at: sqlx::types::chrono::Utc::now().to_rfc3339(),
        source_schema: "public".into(),
        public_schema: "mcp_public".into(),
        scope: Scope {
            instance_id: configured_scope.instance_id().to_owned(),
            discussion_chat_id: configured_scope.discussion_chat_id(),
            source_channel_id: configured_scope.source_channel_id(),
        },
        tables,
    };
    let rendered = toml::to_string_pretty(&manifest)?;
    if let Some(parent) = Path::new(&output).parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, rendered)?;
    println!("wrote {output}");
    Ok(())
}

fn normalize_type(data_type: &str, udt_name: &str) -> anyhow::Result<String> {
    let value = match data_type {
        "ARRAY" => format!("{}[]", normalize_array_type(udt_name)),
        "USER-DEFINED" => udt_name.into(),
        other => other.into(),
    };
    match value.as_str() {
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
        _ => bail!("unsupported mcp_public type {value}"),
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

fn primary_key(name: &str) -> Vec<String> {
    match name {
        "telegram_messages" => vec!["chat_id", "message_id"],
        "telegram_message_edits"
        | "telegram_message_reactions"
        | "post_comment_jobs"
        | "llm_generations"
        | "post_history_entries"
        | "search_runs"
        | "ask_runs"
        | "ask_tool_calls"
        | "telegram_user_notes"
        | "telegram_chat_notes"
        | "voice_transcription_jobs"
        | "admin_events" => vec!["id"],
        "telegram_user_profiles" => vec!["telegram_user_id"],
        "telegram_chat_users"
        | "telegram_chat_member_snapshots"
        | "telegram_new_user_profile_audits" => vec!["chat_id", "telegram_user_id"],
        "telegram_chat_member_events" => vec!["id"],
        "telegram_message_reaction_counts" => vec!["chat_id", "message_id"],
        "telegram_profile_identity_observations" => vec!["telegram_user_id", "snapshot_key"],
        _ => vec!["id"],
    }
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn description(name: &str, instance_id: &str) -> String {
    match name {
        "telegram_messages" => format!("Сообщения публичного чата instance {instance_id}"),
        "telegram_chat_users" => format!("Участники и статистика instance {instance_id}"),
        "ask_runs" => "Публичные запуски /ask".into(),
        "ask_tool_calls" => "Безопасный аудит вызовов /ask".into(),
        "voice_transcription_jobs" => "Расшифровки голосовых публичного чата".into(),
        "post_history_entries" => {
            "Атомарные RAG-карточки публичных постов без исходного текста и embedding".into()
        }
        "telegram_new_user_profile_audits" => "Публичные антиспам-аудиты новых участников".into(),
        other => format!("Проверенная публичная проекция {other}"),
    }
}
