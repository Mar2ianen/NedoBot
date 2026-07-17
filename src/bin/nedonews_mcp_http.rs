//! Public, unauthenticated, read-only MCP server for the NedoNews chat.
//! It deliberately exposes only `mcp_public` projections through a reviewed manifest.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    net::SocketAddr,
    path::Path,
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, bail};
use axum::{
    Json, Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use tracing::info;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;
const MAX_FILTERS: usize = 12;
const MAX_COLUMNS: usize = 40;
const MAX_GROUPS: i64 = 500;
const MAX_TEXT_CHARS: usize = 20_000;

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    manifest: Arc<Manifest>,
    allowed_origins: Arc<BTreeSet<String>>,
    static_avatars_dir: String,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    version: u32,
    source_schema: String,
    public_schema: String,
    scope: ManifestScope,
    tables: BTreeMap<String, ManifestTable>,
}

#[derive(Debug, Deserialize)]
struct ManifestScope {
    discussion_chat_id: i64,
    source_channel_id: i64,
}

#[derive(Debug, Deserialize)]
struct ManifestTable {
    description: String,
    primary_key: Vec<String>,
    #[serde(default)]
    approximate_rows: Option<i64>,
    columns: BTreeMap<String, ManifestColumn>,
}

#[derive(Debug, Deserialize)]
struct ManifestColumn {
    pg_type: String,
    #[serde(default)]
    nullable: bool,
}

#[derive(Deserialize)]
struct JsonRpcRequest {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

#[derive(Deserialize)]
struct ToolCall {
    name: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Deserialize)]
struct DescribeArgs {
    table: String,
}

#[derive(Deserialize)]
struct SelectArgs {
    table: String,
    #[serde(default)]
    columns: Vec<String>,
    #[serde(default)]
    filters: Vec<Filter>,
    #[serde(default)]
    order_by: Vec<OrderBy>,
    limit: Option<i64>,
    cursor: Option<String>,
}

#[derive(Deserialize)]
struct FetchArgs {
    table: String,
    key: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
struct ChatMessageArgs {
    chat_id: i64,
    message_id: i32,
}

#[derive(Deserialize)]
struct UserProfileArgs {
    telegram_user_id: i64,
}

#[derive(Deserialize)]
struct CountArgs {
    table: String,
    #[serde(default)]
    filters: Vec<Filter>,
}

#[derive(Deserialize)]
struct AggregateArgs {
    table: String,
    operation: String,
    column: Option<String>,
    #[serde(default)]
    group_by: Vec<String>,
    #[serde(default)]
    filters: Vec<Filter>,
}

#[derive(Deserialize)]
struct SearchTextArgs {
    table: String,
    column: Option<String>,
    query: String,
    limit: Option<i64>,
    cursor: Option<String>,
}

#[derive(Deserialize)]
struct Filter {
    column: String,
    op: String,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default)]
    values: Vec<Value>,
}

#[derive(Deserialize)]
struct OrderBy {
    column: String,
    direction: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let database_url = env::var("ASK_DATABASE_URL").context("ASK_DATABASE_URL is required")?;
    let manifest_path =
        env::var("MCP_MANIFEST").unwrap_or_else(|_| "config/mcp_db_manifest.toml".into());
    let manifest = Arc::new(load_manifest(&manifest_path)?);
    let pool = readonly_pool(&database_url).await?;
    validate_views(&pool, &manifest).await?;
    let bind = env::var("MCP_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8787".into())
        .parse::<SocketAddr>()?;
    let path = env::var("MCP_PATH").unwrap_or_else(|_| "/mcp/nedonews".into());
    let allowed_origins = env::var("MCP_ALLOWED_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|v| {
            let v = v.trim();
            (!v.is_empty()).then(|| v.to_owned())
        })
        .collect();
    let state = AppState {
        pool,
        manifest,
        allowed_origins: Arc::new(allowed_origins),
        static_avatars_dir: env::var("MCP_STATIC_AVATARS_DIR")
            .unwrap_or_else(|_| "static/avatars".into()),
    };
    let app = Router::new()
        .route(&path, post(mcp_post).get(mcp_get))
        .with_state(state);
    info!(%bind, path = %path, "NedoNews public read-only MCP started");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn load_manifest(path: &str) -> anyhow::Result<Manifest> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read MCP manifest {path}"))?;
    let manifest: Manifest = toml::from_str(&raw).context("invalid MCP manifest TOML")?;
    if manifest.version != 1
        || manifest.source_schema != "public"
        || manifest.public_schema != "mcp_public"
        || manifest.scope.discussion_chat_id != -1001932061163
        || manifest.scope.source_channel_id != -1001575496091
        || manifest.tables.is_empty()
    {
        bail!("invalid MCP manifest metadata");
    }
    for (table, definition) in &manifest.tables {
        ensure_identifier(table)?;
        if definition.columns.is_empty() || definition.primary_key.is_empty() {
            bail!("manifest table {table} has no columns or primary key");
        }
        for (column, field) in &definition.columns {
            ensure_identifier(column)?;
            safe_pg_type(&field.pg_type)?;
        }
    }
    Ok(manifest)
}

async fn readonly_pool(database_url: &str) -> anyhow::Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("set default_transaction_read_only = on")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("set statement_timeout = '5s'")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("set lock_timeout = '1s'")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("set idle_in_transaction_session_timeout = '5s'")
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .context("MCP database connection failed")
}

async fn validate_views(pool: &PgPool, manifest: &Manifest) -> anyhow::Result<()> {
    for (table, expected) in &manifest.tables {
        let rows = sqlx::query("select column_name, data_type, udt_name, is_nullable from information_schema.columns where table_schema = 'mcp_public' and table_name = $1")
            .bind(table).fetch_all(pool).await?;
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
        for (column, expected_column) in &expected.columns {
            if actual.get(column) != Some(&expected_column.pg_type) {
                bail!("MCP manifest drift in {table}.{column}");
            }
        }
        if actual.len() != expected.columns.len() {
            bail!("MCP manifest drift: unreviewed column in {table}");
        }
    }
    Ok(())
}

async fn mcp_get(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if !origin_allowed(&headers, &state.allowed_origins) {
        return StatusCode::FORBIDDEN.into_response();
    }
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(json!({"error": "Use MCP Streamable HTTP POST"})),
    )
        .into_response()
}

async fn mcp_post(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    if !origin_allowed(&headers, &state.allowed_origins) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(id) = request.id.clone() else {
        return StatusCode::ACCEPTED.into_response();
    };
    let started = Instant::now();
    let response = match dispatch(&state, request).await {
        Ok(result) => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        },
        Err(message) => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code: -32602,
                message,
            }),
        },
    };
    info!(client_ip = %peer.ip(), latency_ms = started.elapsed().as_millis(), "MCP request completed");
    Json(response).into_response()
}

fn origin_allowed(headers: &HeaderMap, allowed: &BTreeSet<String>) -> bool {
    let Some(origin) = headers.get("origin") else {
        return true;
    };
    allowed.contains(origin.to_str().unwrap_or_default())
}

async fn dispatch(state: &AppState, request: JsonRpcRequest) -> Result<Value, String> {
    match request.method.as_str() {
        "initialize" => Ok(
            json!({"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"nedonews-readonly-db","version":env!("CARGO_PKG_VERSION")}}),
        ),
        "notifications/initialized" => Ok(Value::Null),
        "tools/list" => Ok(json!({"tools": tools_list()})),
        "tools/call" => {
            let call: ToolCall = serde_json::from_value(request.params)
                .map_err(|_| "invalid tools/call parameters".to_owned())?;
            let value = call_tool(state, &call.name, call.arguments).await?;
            Ok(
                json!({"content":[{"type":"text","text":serde_json::to_string_pretty(&value).map_err(|_| "cannot encode result")?}],"structuredContent":value}),
            )
        }
        _ => Err("unknown MCP method".into()),
    }
}

fn tools_list() -> Vec<Value> {
    let names = [
        (
            "db.list_tables",
            "Возвращает каталог разрешённых публичных view и их primary key. Используй перед любой универсальной DB-выборкой; другие таблицы недоступны.",
        ),
        (
            "db.describe_table",
            "Показывает колонки, типы, primary key и разрешённые операторы одной публичной view. Используй перед db.select, если не знаешь точные поля.",
        ),
        (
            "db.select",
            "Выполняет read-only выборку из разрешённой публичной view с проверенными фильтрами, сортировкой, лимитом и cursor pagination. Не принимает произвольный SQL.",
        ),
        (
            "db.fetch_row",
            "Возвращает одну строку по полному primary key разрешённой view. Для неизвестного сообщения сначала используй поиск.",
        ),
        (
            "db.count",
            "Считает строки разрешённой публичной view по структурированным фильтрам без выгрузки содержимого.",
        ),
        (
            "db.aggregate",
            "Выполняет безопасную count/min/max/sum/avg агрегацию с максимум тремя group_by полями.",
        ),
        (
            "db.search_text",
            "Ищет подстроку в одной разрешённой text-колонке публичной view. Для структурированных фильтров и гибкой сортировки используй db.select.",
        ),
        (
            "chat.search_messages",
            "Ищет текст среди сообщений публичного НедоNews Chat. Возвращает страницы сообщений; для точного сообщения используй chat.get_message.",
        ),
        (
            "chat.get_message",
            "Возвращает одно сообщение публичного Telegram-чата по точной паре chat_id и message_id. Используй после поиска или когда IDs уже известны.",
        ),
        (
            "chat.get_user_profile",
            "Возвращает полную безопасную typed-проекцию публичного профиля участника. Raw payload и Telegram file_id не возвращаются.",
        ),
        (
            "chat.get_user_avatar",
            "Возвращает публичную HTTPS-ссылку на уже кэшированную аватарку участника. Не раскрывает Telegram file_id.",
        ),
        (
            "moderation.list_spammers",
            "Возвращает размеченных спамеров публичного чата, их score, labels и причины. Обычные сообщения чата не возвращает.",
        ),
        (
            "ask.list_runs",
            "Возвращает последние публичные запуски /ask со статусом, временем, моделью и итогом. Используй для аудита работы ассистента, не для поиска сообщений.",
        ),
        (
            "voice.list_transcripts",
            "Расшифровки голосовых из публичного чата.",
        ),
        ("memory.list_notes", "Память публичного канала."),
        ("search.list_runs", "Запуски поиска для комментариев."),
        ("llm.list_generations", "Генерации комментариев."),
    ];
    names
        .into_iter()
        .map(|(name, description)| {
            json!({
                "name": name,
                "description": description,
                "inputSchema": input_schema(name),
                "outputSchema": output_schema(name),
                "annotations": {"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}
            })
        })
        .collect()
}

fn input_schema(name: &str) -> Value {
    let table =
        json!({"type":"string","description":"Имя разрешённой публичной view из db.list_tables."});
    let limit = json!({"type":"integer","minimum":1,"maximum":200,"default":50});
    let cursor =
        json!({"type":["string","null"],"description":"Opaque cursor from the previous page."});
    let filters = json!({"type":"array","maxItems":12,"items":{"type":"object","properties":{"column":{"type":"string"},"op":{"type":"string","enum":["eq","ne","lt","lte","gt","gte","in","not_in","is_null","is_not_null","contains","starts_with","ends_with","between"]},"value":{},"values":{"type":"array","maxItems":100}},"required":["column","op"],"additionalProperties":false}});
    match name {
        "db.list_tables"
        | "ask.list_runs"
        | "voice.list_transcripts"
        | "memory.list_notes"
        | "search.list_runs"
        | "llm.list_generations" => {
            json!({"type":"object","properties":{},"additionalProperties":false})
        }
        "db.describe_table" => {
            json!({"type":"object","properties":{"table":table},"required":["table"],"additionalProperties":false})
        }
        "db.select" => {
            json!({"type":"object","properties":{"table":table,"columns":{"type":"array","maxItems":40,"items":{"type":"string"}},"filters":filters,"order_by":{"type":"array","items":{"type":"object","properties":{"column":{"type":"string"},"direction":{"type":"string","enum":["asc","desc"]}},"required":["column","direction"],"additionalProperties":false}},"limit":limit,"cursor":cursor},"required":["table"],"additionalProperties":false})
        }
        "db.fetch_row" => {
            json!({"type":"object","properties":{"table":table,"key":{"type":"object","additionalProperties":true}},"required":["table","key"],"additionalProperties":false})
        }
        "chat.get_message" => {
            json!({"type":"object","properties":{"chat_id":{"type":"integer","description":"Telegram ID публичного чата; сейчас -1001932061163."},"message_id":{"type":"integer","description":"ID сообщения внутри чата."}},"required":["chat_id","message_id"],"additionalProperties":false})
        }
        "chat.get_user_profile" | "chat.get_user_avatar" => {
            json!({"type":"object","properties":{"telegram_user_id":{"type":"integer","description":"Telegram ID участника публичного чата."}},"required":["telegram_user_id"],"additionalProperties":false})
        }
        "db.count" => {
            json!({"type":"object","properties":{"table":table,"filters":filters},"required":["table"],"additionalProperties":false})
        }
        "db.aggregate" => {
            json!({"type":"object","properties":{"table":table,"operation":{"type":"string","enum":["count","count_distinct","min","max","sum","avg"]},"column":{"type":"string"},"group_by":{"type":"array","maxItems":3,"items":{"type":"string"}},"filters":filters},"required":["table","operation"],"additionalProperties":false})
        }
        "db.search_text" => {
            json!({"type":"object","properties":{"table":table,"column":{"type":"string","description":"Разрешённая text-колонка."},"query":{"type":"string","minLength":1},"limit":limit,"cursor":cursor},"required":["table","column","query"],"additionalProperties":false})
        }
        "chat.search_messages" => {
            json!({"type":"object","properties":{"column":{"type":"string","default":"text"},"query":{"type":"string","minLength":1},"limit":limit,"cursor":cursor},"required":["query"],"additionalProperties":false})
        }
        "moderation.list_spammers" => {
            json!({"type":"object","properties":{},"additionalProperties":false})
        }
        _ => json!({"type":"object","properties":{},"additionalProperties":false}),
    }
}

fn output_schema(name: &str) -> Value {
    let page = json!({"type":"object","properties":{"rows":{"type":"array","items":{"type":"object","additionalProperties":true}},"next_cursor":{"type":["string","null"]},"has_more":{"type":"boolean"}},"required":["rows","next_cursor","has_more"],"additionalProperties":false});
    match name {
        "db.select"
        | "db.search_text"
        | "chat.search_messages"
        | "moderation.list_spammers"
        | "ask.list_runs"
        | "voice.list_transcripts"
        | "memory.list_notes"
        | "search.list_runs"
        | "llm.list_generations" => page,
        "db.fetch_row" => {
            json!({"type":"object","properties":{"row":{"type":["object","null"],"additionalProperties":true}},"required":["row"],"additionalProperties":false})
        }
        "chat.get_message" => {
            json!({"type":"object","properties":{"found":{"type":"boolean"},"chat_id":{"type":"integer"},"message_id":{"type":"integer"},"author_id":{"type":["integer","null"]},"text":{"type":["string","null"]},"created_at":{"type":["string","null"]},"message":{"type":["object","null"],"additionalProperties":true}},"required":["found","chat_id","message_id","author_id","text","created_at","message"],"additionalProperties":false})
        }
        "chat.get_user_profile" => {
            json!({"type":"object","properties":{"found":{"type":"boolean"},"profile":{"type":["object","null"],"additionalProperties":true}},"required":["found","profile"],"additionalProperties":false})
        }
        "chat.get_user_avatar" => {
            json!({"type":"object","properties":{"found":{"type":"boolean"},"telegram_user_id":{"type":"integer"},"avatar_url":{"type":["string","null"],"format":"uri"}},"required":["found","telegram_user_id","avatar_url"],"additionalProperties":false})
        }
        "db.count" => {
            json!({"type":"object","properties":{"count":{"type":"integer"}},"required":["count"],"additionalProperties":false})
        }
        "db.list_tables" => {
            json!({"type":"object","properties":{"tables":{"type":"array","items":{"type":"object","additionalProperties":true}}},"required":["tables"],"additionalProperties":false})
        }
        "db.describe_table" | "db.aggregate" => {
            json!({"type":"object","additionalProperties":true})
        }
        _ => json!({"type":"object","additionalProperties":true}),
    }
}

async fn call_tool(state: &AppState, name: &str, arguments: Value) -> Result<Value, String> {
    match name {
        "db.list_tables" => Ok(list_tables(&state.manifest)),
        "db.describe_table" => {
            let args: DescribeArgs = decode(arguments)?;
            describe_table(&state.manifest, &args.table)
        }
        "db.select" => {
            let args: SelectArgs = decode(arguments)?;
            select_rows(&state.pool, &state.manifest, args).await
        }
        "db.fetch_row" => {
            let args: FetchArgs = decode(arguments)?;
            fetch_row(&state.pool, &state.manifest, args).await
        }
        "db.count" => {
            let args: CountArgs = decode(arguments)?;
            count_rows(&state.pool, &state.manifest, args).await
        }
        "db.aggregate" => {
            let args: AggregateArgs = decode(arguments)?;
            aggregate_rows(&state.pool, &state.manifest, args).await
        }
        "db.search_text" => {
            let args: SearchTextArgs = decode(arguments)?;
            search_text(&state.pool, &state.manifest, args).await
        }
        "chat.search_messages" => {
            let mut arguments = arguments;
            arguments["table"] = Value::String("telegram_messages".into());
            let mut args: SearchTextArgs = decode(arguments)?;
            args.column.get_or_insert("text".into());
            search_text(&state.pool, &state.manifest, args).await
        }
        "chat.get_message" => {
            let args: ChatMessageArgs = decode(arguments)?;
            chat_get_message(&state.pool, &state.manifest, args).await
        }
        "chat.get_user_profile" => {
            let args: UserProfileArgs = decode(arguments)?;
            chat_get_user_profile(&state.pool, &state.manifest, args).await
        }
        "chat.get_user_avatar" => {
            let args: UserProfileArgs = decode(arguments)?;
            chat_get_user_avatar(state, args).await
        }
        "moderation.list_spammers" => {
            select_rows(
                &state.pool,
                &state.manifest,
                SelectArgs {
                    table: "telegram_chat_users".into(),
                    columns: vec![],
                    filters: vec![Filter {
                        column: "is_spammer".into(),
                        op: "eq".into(),
                        value: Some(Value::Bool(true)),
                        values: vec![],
                    }],
                    order_by: vec![OrderBy {
                        column: "spam_score".into(),
                        direction: "desc".into(),
                    }],
                    limit: Some(200),
                    cursor: None,
                },
            )
            .await
        }
        "ask.list_runs" => {
            select_rows(
                &state.pool,
                &state.manifest,
                SelectArgs {
                    table: "ask_runs".into(),
                    columns: vec![],
                    filters: vec![],
                    order_by: vec![OrderBy {
                        column: "created_at".into(),
                        direction: "desc".into(),
                    }],
                    limit: Some(100),
                    cursor: None,
                },
            )
            .await
        }
        "voice.list_transcripts" => {
            select_rows(
                &state.pool,
                &state.manifest,
                SelectArgs {
                    table: "voice_transcription_jobs".into(),
                    columns: vec![],
                    filters: vec![],
                    order_by: vec![OrderBy {
                        column: "created_at".into(),
                        direction: "desc".into(),
                    }],
                    limit: Some(100),
                    cursor: None,
                },
            )
            .await
        }
        "memory.list_notes" => {
            select_rows(
                &state.pool,
                &state.manifest,
                SelectArgs {
                    table: "post_memory_notes".into(),
                    columns: vec![],
                    filters: vec![],
                    order_by: vec![OrderBy {
                        column: "created_at".into(),
                        direction: "desc".into(),
                    }],
                    limit: Some(100),
                    cursor: None,
                },
            )
            .await
        }
        "search.list_runs" => {
            select_rows(
                &state.pool,
                &state.manifest,
                SelectArgs {
                    table: "search_runs".into(),
                    columns: vec![],
                    filters: vec![],
                    order_by: vec![OrderBy {
                        column: "created_at".into(),
                        direction: "desc".into(),
                    }],
                    limit: Some(100),
                    cursor: None,
                },
            )
            .await
        }
        "llm.list_generations" => {
            select_rows(
                &state.pool,
                &state.manifest,
                SelectArgs {
                    table: "llm_generations".into(),
                    columns: vec![],
                    filters: vec![],
                    order_by: vec![OrderBy {
                        column: "created_at".into(),
                        direction: "desc".into(),
                    }],
                    limit: Some(100),
                    cursor: None,
                },
            )
            .await
        }
        _ => Err("unknown read-only tool".into()),
    }
}

fn decode<T: for<'a> Deserialize<'a>>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|_| "invalid tool arguments".into())
}

fn table<'a>(manifest: &'a Manifest, name: &str) -> Result<&'a ManifestTable, String> {
    manifest
        .tables
        .get(name)
        .ok_or_else(|| "unknown table".into())
}
fn column<'a>(table: &'a ManifestTable, name: &str) -> Result<&'a ManifestColumn, String> {
    table
        .columns
        .get(name)
        .ok_or_else(|| "unknown column".into())
}

fn list_tables(manifest: &Manifest) -> Value {
    json!({"tables": manifest.tables.iter().map(|(name, table)| json!({"name":name,"description":table.description,"primary_key":table.primary_key,"approximate_rows":table.approximate_rows})).collect::<Vec<_>>()})
}
fn describe_table(manifest: &Manifest, name: &str) -> Result<Value, String> {
    let table = table(manifest, name)?;
    Ok(
        json!({"name":name,"description":table.description,"primary_key":table.primary_key,"columns":table.columns.iter().map(|(name, col)| json!({"name":name,"type":col.pg_type,"nullable":col.nullable})).collect::<Vec<_>>(),"filter_operators":["eq","ne","lt","lte","gt","gte","in","not_in","is_null","is_not_null","contains","starts_with","ends_with","between"],"max_limit":MAX_LIMIT}),
    )
}

async fn select_rows(
    pool: &PgPool,
    manifest: &Manifest,
    args: SelectArgs,
) -> Result<Value, String> {
    if args.filters.len() > MAX_FILTERS || args.columns.len() > MAX_COLUMNS {
        return Err("too many filters or columns".into());
    }
    let definition = table(manifest, &args.table)?;
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err("limit must be between 1 and 200".into());
    }
    let offset = decode_cursor(args.cursor.as_deref())?;
    let columns = if args.columns.is_empty() {
        definition.columns.keys().cloned().collect::<Vec<_>>()
    } else {
        args.columns
    };
    for name in &columns {
        column(definition, name)?;
    }
    let mut sql = format!(
        "select {} from mcp_public.{}",
        select_list(definition, &columns)?,
        args.table
    );
    let mut binds = Vec::new();
    append_filters(&mut sql, definition, &args.filters, &mut binds)?;
    if args.order_by.is_empty() {
        sql.push_str(&format!(
            " order by {}",
            definition
                .primary_key
                .iter()
                .map(|v| quote(v))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    } else {
        append_order(&mut sql, definition, &args.order_by)?;
    }
    sql.push_str(&format!(" limit {} offset {}", limit + 1, offset));
    let rows = run_json_rows(pool, &sql, &binds).await?;
    let has_more = rows.len() as i64 > limit;
    let rows = rows
        .into_iter()
        .take(limit as usize)
        .map(sanitize_value)
        .collect::<Vec<_>>();
    info!(tool="db.select", table=%args.table, result_count=rows.len(), "MCP database tool completed");
    Ok(
        json!({"rows":rows,"next_cursor":has_more.then(|| encode_cursor(offset + limit)),"has_more":has_more}),
    )
}

async fn fetch_row(pool: &PgPool, manifest: &Manifest, args: FetchArgs) -> Result<Value, String> {
    let definition = table(manifest, &args.table)?;
    if args.key.len() != definition.primary_key.len()
        || !definition
            .primary_key
            .iter()
            .all(|key| args.key.contains_key(key))
    {
        return Err("key must contain exactly the full primary key".into());
    }
    let filters = definition
        .primary_key
        .iter()
        .map(|key| Filter {
            column: key.clone(),
            op: "eq".into(),
            value: args.key.get(key).cloned(),
            values: vec![],
        })
        .collect();
    let result = select_rows(
        pool,
        manifest,
        SelectArgs {
            table: args.table,
            columns: vec![],
            filters,
            order_by: vec![],
            limit: Some(1),
            cursor: None,
        },
    )
    .await?;
    Ok(json!({"row":result["rows"].as_array().and_then(|rows| rows.first()).cloned()}))
}

async fn chat_get_message(
    pool: &PgPool,
    manifest: &Manifest,
    args: ChatMessageArgs,
) -> Result<Value, String> {
    if args.chat_id != -1001932061163 {
        return Err("chat_id is outside the public chat scope".into());
    }
    let result = fetch_row(
        pool,
        manifest,
        FetchArgs {
            table: "telegram_messages".into(),
            key: BTreeMap::from([
                ("chat_id".into(), Value::from(args.chat_id)),
                ("message_id".into(), Value::from(args.message_id)),
            ]),
        },
    )
    .await?;
    let message = result["row"].clone();
    let found = !message.is_null();
    Ok(json!({
        "found": found,
        "chat_id": args.chat_id,
        "message_id": args.message_id,
        "author_id": message["user_id"].clone(),
        "text": message["text"].clone(),
        "created_at": message["created_at"].clone(),
        "message": message,
    }))
}

async fn chat_get_user_profile(
    pool: &PgPool,
    manifest: &Manifest,
    args: UserProfileArgs,
) -> Result<Value, String> {
    let result = fetch_row(
        pool,
        manifest,
        FetchArgs {
            table: "telegram_user_profiles".into(),
            key: BTreeMap::from([(
                "telegram_user_id".into(),
                Value::from(args.telegram_user_id),
            )]),
        },
    )
    .await?;
    let profile = result["row"].clone();
    Ok(json!({"found": !profile.is_null(), "profile": profile}))
}

async fn chat_get_user_avatar(state: &AppState, args: UserProfileArgs) -> Result<Value, String> {
    let row = sqlx::query("select profile_photo_file_unique_id from mcp_public.telegram_user_profiles where telegram_user_id = $1")
        .bind(args.telegram_user_id).fetch_optional(&state.pool).await.map_err(|_| "database query failed")?;
    let unique_id = row.and_then(|row| {
        row.try_get::<Option<String>, _>("profile_photo_file_unique_id")
            .ok()
            .flatten()
    });
    let filename = unique_id.map(|unique_id| {
        format!(
            "{}_{}.jpg",
            args.telegram_user_id,
            safe_static_name(&unique_id)
        )
    });
    let avatar_url = filename
        .filter(|name| Path::new(&state.static_avatars_dir).join(name).is_file())
        .map(|name| format!("https://nedobot.chickenkiller.com/tg-ai-bot-static/avatars/{name}"));
    Ok(
        json!({"found": avatar_url.is_some(), "telegram_user_id": args.telegram_user_id, "avatar_url": avatar_url}),
    )
}

fn safe_static_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .collect()
}

async fn count_rows(pool: &PgPool, manifest: &Manifest, args: CountArgs) -> Result<Value, String> {
    if args.filters.len() > MAX_FILTERS {
        return Err("too many filters".into());
    }
    let definition = table(manifest, &args.table)?;
    let mut sql = format!(
        "select count(*)::bigint as count from mcp_public.{}",
        args.table
    );
    let mut binds = Vec::new();
    append_filters(&mut sql, definition, &args.filters, &mut binds)?;
    let row = bind_all(sqlx::query(&sql), &binds)
        .fetch_one(pool)
        .await
        .map_err(|_| "database query failed".to_owned())?;
    let count: i64 = row.try_get("count").map_err(|_| "database result failed")?;
    Ok(json!({"count":count}))
}

async fn aggregate_rows(
    pool: &PgPool,
    manifest: &Manifest,
    args: AggregateArgs,
) -> Result<Value, String> {
    if args.group_by.len() > 3 || args.filters.len() > MAX_FILTERS {
        return Err("too many grouping columns or filters".into());
    }
    let definition = table(manifest, &args.table)?;
    let expression = match args.operation.as_str() {
        "count" => "count(*)".to_owned(),
        "count_distinct" => format!(
            "count(distinct {})",
            quote(
                args.column
                    .as_deref()
                    .ok_or("aggregate column is required")?
            )
        ),
        "min" | "max" | "sum" | "avg" => format!(
            "{}({})",
            args.operation,
            quote(
                args.column
                    .as_deref()
                    .ok_or("aggregate column is required")?
            )
        ),
        _ => return Err("unknown aggregate operation".into()),
    };
    if let Some(column_name) = &args.column {
        column(definition, column_name)?;
    }
    for name in &args.group_by {
        column(definition, name)?;
    }
    let groups = args
        .group_by
        .iter()
        .map(|v| quote(v))
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = if groups.is_empty() {
        format!(
            "select {} as value from mcp_public.{}",
            expression, args.table
        )
    } else {
        format!(
            "select {}, {} as value from mcp_public.{}",
            groups, expression, args.table
        )
    };
    let mut binds = Vec::new();
    append_filters(&mut sql, definition, &args.filters, &mut binds)?;
    if !groups.is_empty() {
        sql.push_str(&format!(" group by {} limit {}", groups, MAX_GROUPS));
    }
    let rows = run_json_rows(pool, &sql, &binds).await?;
    Ok(json!({"rows":rows.into_iter().map(sanitize_value).collect::<Vec<_>>() }))
}

async fn search_text(
    pool: &PgPool,
    manifest: &Manifest,
    args: SearchTextArgs,
) -> Result<Value, String> {
    let definition = table(manifest, &args.table)?;
    let column_name = args.column.unwrap_or_else(|| "text".into());
    let field = column(definition, &column_name)?;
    if field.pg_type != "text" {
        return Err("search_text requires a text column".into());
    }
    select_rows(
        pool,
        manifest,
        SelectArgs {
            table: args.table,
            columns: vec![],
            filters: vec![Filter {
                column: column_name,
                op: "contains".into(),
                value: Some(Value::String(args.query)),
                values: vec![],
            }],
            order_by: vec![],
            limit: args.limit,
            cursor: args.cursor,
        },
    )
    .await
}

fn select_list(definition: &ManifestTable, columns: &[String]) -> Result<String, String> {
    columns
        .iter()
        .map(|name| {
            let field = column(definition, name)?;
            let quoted = quote(name);
            Ok(if field.pg_type == "text" {
                format!("left({}, {}) as {}", quoted, MAX_TEXT_CHARS, quoted)
            } else {
                quoted
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|v| v.join(", "))
}
fn append_order(
    sql: &mut String,
    definition: &ManifestTable,
    order: &[OrderBy],
) -> Result<(), String> {
    let values = order
        .iter()
        .map(|item| {
            column(definition, &item.column)?;
            let direction = match item.direction.as_str() {
                "asc" => "asc",
                "desc" => "desc",
                _ => return Err("order direction must be asc or desc".into()),
            };
            Ok(format!("{} {}", quote(&item.column), direction))
        })
        .collect::<Result<Vec<_>, String>>()?;
    sql.push_str(" order by ");
    sql.push_str(&values.join(", "));
    Ok(())
}
fn append_filters(
    sql: &mut String,
    definition: &ManifestTable,
    filters: &[Filter],
    binds: &mut Vec<String>,
) -> Result<(), String> {
    if filters.is_empty() {
        return Ok(());
    }
    sql.push_str(" where ");
    let mut parts = Vec::new();
    for filter in filters {
        let field = column(definition, &filter.column)?;
        let name = quote(&filter.column);
        let cast = safe_pg_type(&field.pg_type).map_err(|_| "unsupported manifest column type")?;
        let single = |value: &Option<Value>, binds: &mut Vec<String>| -> Result<String, String> {
            let value = value.as_ref().ok_or("filter value is required")?;
            binds.push(value_to_text(value)?);
            Ok(format!("${}::{}", binds.len(), cast))
        };
        let part = match filter.op.as_str() {
            "eq" => format!("{} = {}", name, single(&filter.value, binds)?),
            "ne" => format!("{} <> {}", name, single(&filter.value, binds)?),
            "lt" => format!("{} < {}", name, single(&filter.value, binds)?),
            "lte" => format!("{} <= {}", name, single(&filter.value, binds)?),
            "gt" => format!("{} > {}", name, single(&filter.value, binds)?),
            "gte" => format!("{} >= {}", name, single(&filter.value, binds)?),
            "is_null" => format!("{} is null", name),
            "is_not_null" => format!("{} is not null", name),
            "contains" => {
                binds.push(format!(
                    "%{}%",
                    value_to_text(filter.value.as_ref().ok_or("filter value is required")?)?
                ));
                format!("{}::text ilike ${}", name, binds.len())
            }
            "starts_with" => {
                binds.push(format!(
                    "{}%",
                    value_to_text(filter.value.as_ref().ok_or("filter value is required")?)?
                ));
                format!("{}::text ilike ${}", name, binds.len())
            }
            "ends_with" => {
                binds.push(format!(
                    "%{}",
                    value_to_text(filter.value.as_ref().ok_or("filter value is required")?)?
                ));
                format!("{}::text ilike ${}", name, binds.len())
            }
            "between" => {
                if filter.values.len() != 2 {
                    return Err("between requires two values".into());
                };
                binds.push(value_to_text(&filter.values[0])?);
                let first = binds.len();
                binds.push(value_to_text(&filter.values[1])?);
                format!(
                    "{} between ${}::{} and ${}::{}",
                    name,
                    first,
                    cast,
                    binds.len(),
                    cast
                )
            }
            "in" | "not_in" => {
                if filter.values.is_empty() || filter.values.len() > 100 {
                    return Err("in requires 1 to 100 values".into());
                };
                let placeholders = filter
                    .values
                    .iter()
                    .map(|value| {
                        binds.push(value_to_text(value)?);
                        Ok(format!("${}::{}", binds.len(), cast))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                format!(
                    "{} {} in ({})",
                    name,
                    if filter.op == "not_in" { "not" } else { "" },
                    placeholders.join(", ")
                )
            }
            _ => return Err("unknown filter operator".into()),
        };
        parts.push(part);
    }
    sql.push_str(&parts.join(" and "));
    Ok(())
}

async fn run_json_rows(pool: &PgPool, sql: &str, binds: &[String]) -> Result<Vec<Value>, String> {
    let wrapped = format!("select to_jsonb(result_row) as row from ({sql}) result_row");
    let rows = bind_all(sqlx::query(&wrapped), binds)
        .fetch_all(pool)
        .await
        .map_err(|_| "database query failed".to_owned())?;
    rows.into_iter()
        .map(|row| {
            row.try_get::<sqlx::types::Json<Value>, _>("row")
                .map(|value| value.0)
                .map_err(|_| "database result encoding failed".to_owned())
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
fn quote(value: &str) -> String {
    format!("\"{}\"", value)
}
fn value_to_text(value: &Value) -> Result<String, String> {
    match value {
        Value::String(v) => Ok(v.clone()),
        Value::Number(v) => Ok(v.to_string()),
        Value::Bool(v) => Ok(v.to_string()),
        Value::Null => Err("null must use is_null".into()),
        _ => serde_json::to_string(value).map_err(|_| "invalid filter value".into()),
    }
}
fn encode_cursor(offset: i64) -> String {
    URL_SAFE_NO_PAD.encode(offset.to_be_bytes())
}
fn decode_cursor(cursor: Option<&str>) -> Result<i64, String> {
    let Some(cursor) = cursor else { return Ok(0) };
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| "invalid cursor")?;
    if bytes.len() != 8 {
        return Err("invalid cursor".into());
    };
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes);
    let value = i64::from_be_bytes(raw);
    if value < 0 {
        Err("invalid cursor".into())
    } else {
        Ok(value)
    }
}
fn ensure_identifier(value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        bail!("unsafe identifier in MCP manifest")
    };
    Ok(())
}
fn safe_pg_type(value: &str) -> anyhow::Result<&str> {
    match value {
        "bigint"
        | "integer"
        | "smallint"
        | "double precision"
        | "boolean"
        | "text"
        | "timestamp with time zone"
        | "jsonb"
        | "text[]" => Ok(value),
        _ => bail!("unsupported PostgreSQL type in MCP manifest"),
    }
}
fn normalize_pg_type(data_type: &str, udt_name: &str) -> String {
    match data_type {
        "ARRAY" => format!("{}[]", udt_name.trim_start_matches('_')),
        "USER-DEFINED" => udt_name.into(),
        _ => data_type.into(),
    }
}
fn sanitize_value(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(sanitize_value).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    let sensitive = [
                        "token",
                        "access_token",
                        "refresh_token",
                        "api_key",
                        "apikey",
                        "secret",
                        "password",
                        "passwd",
                        "authorization",
                        "cookie",
                        "set-cookie",
                        "database_url",
                        "dsn",
                        "private_key",
                        "client_secret",
                        "webhook_secret",
                        "invite_link",
                        "signed_url",
                    ];
                    let value = if sensitive
                        .iter()
                        .any(|needle| key.eq_ignore_ascii_case(needle))
                    {
                        Value::String("<redacted>".into())
                    } else {
                        sanitize_value(value)
                    };
                    (key, value)
                })
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_round_trip() {
        assert_eq!(decode_cursor(Some(&encode_cursor(42))).unwrap(), 42)
    }
    #[test]
    fn sanitizer_hides_nested_secret() {
        let result = sanitize_value(json!({"a":{"API_KEY":"x"}}));
        assert_eq!(result["a"]["API_KEY"], "<redacted>")
    }
    #[test]
    fn manifest_refuses_injection_identifier() {
        assert!(ensure_identifier("messages;drop table").is_err())
    }
    #[test]
    fn parameterized_tools_publish_input_and_output_schemas() {
        let tools = tools_list();
        let find = |name| tools.iter().find(|tool| tool["name"] == name).unwrap();
        let search = find("db.search_text");
        assert!(search["inputSchema"]["properties"]["query"].is_object());
        assert!(
            search["inputSchema"]["required"]
                .as_array()
                .unwrap()
                .contains(&Value::String("query".into()))
        );
        assert!(search["outputSchema"]["properties"]["rows"].is_object());
        assert!(find("db.select")["inputSchema"]["properties"]["table"].is_object());
        let message = find("chat.get_message");
        assert!(message["inputSchema"]["properties"]["chat_id"].is_object());
        assert!(message["inputSchema"]["properties"]["message_id"].is_object());
        assert!(message["outputSchema"]["properties"]["message_id"].is_object());
    }
}
