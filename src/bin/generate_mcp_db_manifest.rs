//! Generates the reviewed MCP manifest from `mcp_public` views.
//! Run only after inspecting the resulting diff before publishing a new view/column.

use std::{collections::BTreeMap, env, fs, path::Path};

use anyhow::{Context, bail};
use serde::Serialize;
use sqlx::{Row, postgres::PgPoolOptions};

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
    discussion_chat_id: i64,
    source_channel_id: i64,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let database_url = env::var("DATABASE_URL")
        .or_else(|_| env::var("ASK_DATABASE_URL"))
        .context("DATABASE_URL or ASK_DATABASE_URL is required")?;
    let output = env::args()
        .nth(1)
        .unwrap_or_else(|| "config/mcp_db_manifest.toml".into());
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;
    let rows = sqlx::query("select table_name from information_schema.views where table_schema = 'mcp_public' order by table_name").fetch_all(&pool).await?;
    if rows.is_empty() {
        bail!("mcp_public has no views; apply the MCP view migration first");
    }
    let mut tables = BTreeMap::new();
    for row in rows {
        let name: String = row.get("table_name");
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
                description: description(&name),
                primary_key: primary_key(&name),
                approximate_rows: estimate,
                columns,
            },
        );
    }
    let manifest = Manifest {
        version: 1,
        generated_at: sqlx::types::chrono::Utc::now().to_rfc3339(),
        source_schema: "public".into(),
        public_schema: "mcp_public".into(),
        scope: Scope {
            discussion_chat_id: -1001932061163,
            source_channel_id: -1001575496091,
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
        | "avatar_analysis_jobs"
        | "admin_events" => vec!["id"],
        "telegram_user_profiles" => vec!["telegram_user_id"],
        "telegram_chat_users"
        | "telegram_chat_member_snapshots"
        | "telegram_new_user_profile_audits" => vec!["chat_id", "telegram_user_id"],
        "telegram_chat_member_events" => vec!["id"],
        "telegram_message_reaction_counts" => vec!["chat_id", "message_id"],
        "telegram_profile_identity_observations" => vec!["telegram_user_id", "snapshot_key"],
        "avatar_profile_assessments" => vec![
            "telegram_user_id",
            "profile_photo_file_unique_id",
            "features_snapshot_hash",
            "prompt_version",
        ],
        "avatar_image_analyses" => vec!["profile_photo_file_unique_id", "prompt_version"],
        _ => vec!["id"],
    }
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn description(name: &str) -> String {
    match name {
        "telegram_messages" => "Сообщения публичного НедоNews Chat".into(),
        "telegram_chat_users" => "Участники и статистика публичного чата".into(),
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
