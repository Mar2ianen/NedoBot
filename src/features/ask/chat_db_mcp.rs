use std::env;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::features::ask::chat_search::{
    MessageSearchRequest, MessageSort, message_context, search_messages,
};

const TOOL_SEARCH_MESSAGES: &str = "chat.search_messages";
const TOOL_MESSAGE_CONTEXT: &str = "chat.get_message_context";
const TOOL_LIST_CHAT_NOTES: &str = "notes.list_chat";
const TOOL_LIST_USER_NOTES: &str = "notes.list_user";

#[derive(Deserialize)]
struct JsonRpcRequest {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Deserialize)]
struct SearchArguments {
    query: String,
    user_id: Option<i64>,
    date_from: Option<String>,
    date_to: Option<String>,
    reply_to_message_id: Option<i32>,
    has_links: Option<bool>,
    has_media: Option<bool>,
    #[serde(default)]
    sort: Option<MessageSort>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct ContextArguments {
    message_id: i32,
    #[serde(default)]
    before: Option<i64>,
    #[serde(default)]
    after: Option<i64>,
}

#[derive(Deserialize)]
struct UserNotesArguments {
    telegram_user_id: i64,
}

#[derive(Serialize, sqlx::FromRow)]
struct NoteRow {
    id: i64,
    note: String,
    created_by_user_id: i64,
    created_at: String,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: &'static str,
}

pub async fn run_stdio_server() -> anyhow::Result<()> {
    let database_url = env::var("ASK_DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("ASK_DATABASE_URL is required for chat_db_mcp"))?;
    let chat_id = env::var("DISCUSSION_CHAT_ID")
        .map_err(|_| anyhow::anyhow!("DISCUSSION_CHAT_ID is required for chat_db_mcp"))?
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("DISCUSSION_CHAT_ID must be an integer"))?;
    let pool = build_readonly_pool(&database_url).await?;

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::BufWriter::new(stdout);

    while let Some(line) = lines.next_line().await? {
        let Ok(request) = serde_json::from_str::<JsonRpcRequest>(&line) else {
            continue;
        };
        let Some(id) = request.id.clone() else {
            continue;
        };

        let response = handle_request(&pool, chat_id, request, id).await;
        let encoded = serde_json::to_string(&response)?;
        stdout.write_all(encoded.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }

    Ok(())
}

async fn build_readonly_pool(database_url: &str) -> anyhow::Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _meta| {
            Box::pin(async move {
                sqlx::query("set default_transaction_read_only = on")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("set statement_timeout = '5000ms'")
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .map_err(|_| anyhow::anyhow!("chat DB MCP connection failed"))
}

async fn handle_request(
    pool: &PgPool,
    chat_id: i64,
    request: JsonRpcRequest,
    id: Value,
) -> JsonRpcResponse {
    let result = match request.method.as_str() {
        "initialize" => Ok(initialize_result()),
        "tools/list" => Ok(tools_list_result()),
        "tools/call" => call_tool(pool, chat_id, request.params).await,
        _ => Err(()),
    };

    match result {
        Ok(result) => success(id, result),
        Err(()) => failure(id),
    }
}

async fn call_tool(pool: &PgPool, chat_id: i64, params: Value) -> Result<Value, ()> {
    let params: ToolCallParams = serde_json::from_value(params).map_err(|_| ())?;
    match params.name.as_str() {
        TOOL_SEARCH_MESSAGES => {
            let arguments: SearchArguments =
                serde_json::from_value(params.arguments).map_err(|_| ())?;
            let date_from = parse_timestamp(arguments.date_from)?;
            let date_to = parse_timestamp(arguments.date_to)?;
            let messages = search_messages(
                pool,
                &MessageSearchRequest {
                    chat_id,
                    query: arguments.query,
                    user_id: arguments.user_id,
                    date_from,
                    date_to,
                    reply_to_message_id: arguments.reply_to_message_id,
                    has_links: arguments.has_links,
                    has_media: arguments.has_media,
                    sort: arguments.sort.unwrap_or(MessageSort::Relevance),
                    limit: arguments.limit.unwrap_or(10),
                },
            )
            .await
            .map_err(|_| ())?;
            tool_text_result(&messages)
        }
        TOOL_MESSAGE_CONTEXT => {
            let arguments: ContextArguments =
                serde_json::from_value(params.arguments).map_err(|_| ())?;
            let messages = message_context(
                pool,
                chat_id,
                arguments.message_id,
                arguments.before.unwrap_or(3),
                arguments.after.unwrap_or(3),
            )
            .await
            .map_err(|_| ())?;
            tool_text_result(&messages)
        }
        TOOL_LIST_CHAT_NOTES => {
            let notes = sqlx::query_as::<_, NoteRow>("select id, note, created_by_user_id, created_at::text as created_at from telegram_chat_notes where chat_id = $1 and status = 'active' order by created_at desc limit 20")
                .bind(chat_id).fetch_all(pool).await.map_err(|_| ())?;
            tool_text_result(&notes)
        }
        TOOL_LIST_USER_NOTES => {
            let arguments: UserNotesArguments =
                serde_json::from_value(params.arguments).map_err(|_| ())?;
            let notes = sqlx::query_as::<_, NoteRow>("select id, note, created_by_user_id, created_at::text as created_at from telegram_user_notes where chat_id = $1 and telegram_user_id = $2 and status = 'active' order by created_at desc limit 20")
                .bind(chat_id).bind(arguments.telegram_user_id).fetch_all(pool).await.map_err(|_| ())?;
            tool_text_result(&notes)
        }
        _ => Err(()),
    }
}

fn parse_timestamp(value: Option<String>) -> Result<Option<DateTime<Utc>>, ()> {
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(|timestamp| timestamp.with_timezone(&Utc))
                .map_err(|_| ())
        })
        .transpose()
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "chat-db-mcp", "version": "0.1.0"}
    })
}

fn tools_list_result() -> Value {
    json!({"tools": [
        {"name": TOOL_SEARCH_MESSAGES, "description": "Ищет сообщения только в разрешённом чате.", "inputSchema": {"type": "object", "additionalProperties": false, "required": ["query"], "properties": {"query": {"type": "string", "maxLength": 240}, "user_id": {"type": "integer"}, "date_from": {"type": "string", "format": "date-time"}, "date_to": {"type": "string", "format": "date-time"}, "reply_to_message_id": {"type": "integer"}, "has_links": {"type": "boolean"}, "has_media": {"type": "boolean"}, "sort": {"type": "string", "enum": ["relevance", "newest", "oldest"]}, "limit": {"type": "integer", "minimum": 1, "maximum": 20}}}},
        {"name": TOOL_MESSAGE_CONTEXT, "description": "Возвращает ограниченный контекст вокруг найденного сообщения.", "inputSchema": {"type": "object", "additionalProperties": false, "required": ["message_id"], "properties": {"message_id": {"type": "integer"}, "before": {"type": "integer", "minimum": 0, "maximum": 5}, "after": {"type": "integer", "minimum": 0, "maximum": 5}}}}
        ,{"name": TOOL_LIST_CHAT_NOTES, "description": "Возвращает активные общие заметки разрешённого чата.", "inputSchema": {"type": "object", "additionalProperties": false, "properties": {}}}
        ,{"name": TOOL_LIST_USER_NOTES, "description": "Возвращает активные заметки указанного пользователя в разрешённом чате.", "inputSchema": {"type": "object", "additionalProperties": false, "required": ["telegram_user_id"], "properties": {"telegram_user_id": {"type": "integer"}}}}
    ]})
}

fn tool_text_result<T: Serialize>(value: &T) -> Result<Value, ()> {
    let text = serde_json::to_string(value).map_err(|_| ())?;
    Ok(json!({"content": [{"type": "text", "text": text}], "isError": false}))
}

fn success(id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    }
}

fn failure(id: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code: -32602,
            message: "invalid or failed chat DB MCP request",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_only_read_tools() {
        let tools = tools_list_result();
        let names = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                TOOL_SEARCH_MESSAGES,
                TOOL_MESSAGE_CONTEXT,
                TOOL_LIST_CHAT_NOTES,
                TOOL_LIST_USER_NOTES,
            ]
        );
    }

    #[test]
    fn failure_hides_database_errors() {
        let encoded = serde_json::to_string(&failure(json!(1))).unwrap();
        assert!(!encoded.contains("postgres"));
        assert!(!encoded.contains("DATABASE_URL"));
    }
}
