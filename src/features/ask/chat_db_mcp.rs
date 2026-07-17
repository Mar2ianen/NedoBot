use std::env;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::features::ask::chat_search::{
    MessageMatch, MessageSearchRequest, MessageSort, RecentMessagesRequest, message_context,
    recent_messages, reply_thread, search_messages, user_interactions, user_profile,
};

const TOOL_SEARCH_MESSAGES: &str = "chat.search_messages";
const TOOL_SEARCH_MESSAGES_BATCH: &str = "chat.search_messages_batch";
const TOOL_RECENT_MESSAGES: &str = "chat.get_recent_messages";
const TOOL_GET_MESSAGE: &str = "chat.get_message";
const TOOL_MESSAGE_CONTEXT: &str = "chat.get_message_context";
const TOOL_REPLY_THREAD: &str = "chat.get_reply_thread";
const TOOL_RESOLVE_USER: &str = "chat.resolve_user";
const TOOL_USER_INTERACTIONS: &str = "chat.get_user_interactions";
const TOOL_USER_PROFILE: &str = "chat.get_user_profile";
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
    match_mode: Option<MessageMatch>,
    #[serde(default)]
    sort: Option<MessageSort>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct RecentArguments {
    user_id: Option<i64>,
    date_from: Option<String>,
    date_to: Option<String>,
    has_links: Option<bool>,
    has_media: Option<bool>,
    #[serde(default)]
    sort: Option<MessageSort>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct BatchSearchArguments {
    queries: Vec<String>,
    user_id: Option<i64>,
    date_from: Option<String>,
    date_to: Option<String>,
    has_links: Option<bool>,
    has_media: Option<bool>,
    #[serde(default)]
    match_mode: Option<MessageMatch>,
    #[serde(default)]
    sort: Option<MessageSort>,
    #[serde(default)]
    limit_per_query: Option<i64>,
}

#[derive(Serialize)]
struct BatchSearchResult {
    query: String,
    messages: Value,
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

#[derive(Deserialize)]
struct ResolveUserArguments {
    query: Option<String>,
    telegram_user_id: Option<i64>,
}

#[derive(Deserialize)]
struct UserInteractionsArguments {
    first_user_id: i64,
    second_user_id: i64,
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct UserProfileArguments {
    telegram_user_id: i64,
}

#[derive(Serialize, sqlx::FromRow)]
struct ResolvedUserRow {
    telegram_user_id: i64,
    username: Option<String>,
    display_name: String,
    match_rank: i32,
    message_count: i64,
}

#[derive(Serialize, sqlx::FromRow)]
struct FuzzyUserRow {
    telegram_user_id: i64,
    username: Option<String>,
    display_name: String,
    message_count: i64,
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
                    match_mode: arguments.match_mode.unwrap_or(MessageMatch::FullText),
                    sort: arguments.sort.unwrap_or(MessageSort::Relevance),
                    limit: arguments.limit.unwrap_or(10),
                },
            )
            .await
            .map_err(|_| ())?;
            tool_text_result(&messages)
        }
        TOOL_SEARCH_MESSAGES_BATCH => {
            let arguments: BatchSearchArguments =
                serde_json::from_value(params.arguments).map_err(|_| ())?;
            if arguments.queries.is_empty() {
                return Err(());
            }
            let date_from = parse_timestamp(arguments.date_from)?;
            let date_to = parse_timestamp(arguments.date_to)?;
            let queries = arguments.queries.into_iter().take(6).collect::<Vec<_>>();
            let mut results = Vec::with_capacity(queries.len());
            for query in queries {
                let messages = search_messages(
                    pool,
                    &MessageSearchRequest {
                        chat_id,
                        query: query.clone(),
                        user_id: arguments.user_id,
                        date_from,
                        date_to,
                        reply_to_message_id: None,
                        has_links: arguments.has_links,
                        has_media: arguments.has_media,
                        match_mode: arguments
                            .match_mode
                            .clone()
                            .unwrap_or(MessageMatch::FullText),
                        sort: arguments.sort.clone().unwrap_or(MessageSort::Relevance),
                        limit: arguments.limit_per_query.unwrap_or(5).clamp(1, 5),
                    },
                )
                .await
                .map_err(|_| ())?;
                results.push(BatchSearchResult {
                    query,
                    messages: serde_json::to_value(messages).map_err(|_| ())?,
                });
            }
            tool_text_result(&results)
        }
        TOOL_RECENT_MESSAGES => {
            let arguments: RecentArguments =
                serde_json::from_value(params.arguments).map_err(|_| ())?;
            let messages = recent_messages(
                pool,
                &RecentMessagesRequest {
                    chat_id,
                    user_id: arguments.user_id,
                    date_from: parse_timestamp(arguments.date_from)?,
                    date_to: parse_timestamp(arguments.date_to)?,
                    has_links: arguments.has_links,
                    has_media: arguments.has_media,
                    sort: arguments.sort.unwrap_or(MessageSort::Newest),
                    limit: arguments.limit.unwrap_or(20),
                },
            )
            .await
            .map_err(|_| ())?;
            tool_text_result(&messages)
        }
        TOOL_GET_MESSAGE => {
            let arguments: ContextArguments =
                serde_json::from_value(params.arguments).map_err(|_| ())?;
            tool_text_result(
                &message_context(pool, chat_id, arguments.message_id, 0, 0)
                    .await
                    .map_err(|_| ())?,
            )
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
        TOOL_REPLY_THREAD => {
            let arguments: ContextArguments =
                serde_json::from_value(params.arguments).map_err(|_| ())?;
            tool_text_result(
                &reply_thread(pool, chat_id, arguments.message_id)
                    .await
                    .map_err(|_| ())?,
            )
        }
        TOOL_RESOLVE_USER => resolve_user(pool, chat_id, params.arguments).await,
        TOOL_USER_INTERACTIONS => {
            let arguments: UserInteractionsArguments =
                serde_json::from_value(params.arguments).map_err(|_| ())?;
            if arguments.first_user_id == arguments.second_user_id {
                return Err(());
            }
            tool_text_result(
                &user_interactions(
                    pool,
                    chat_id,
                    arguments.first_user_id,
                    arguments.second_user_id,
                    arguments.limit.unwrap_or(20),
                )
                .await
                .map_err(|_| ())?,
            )
        }
        TOOL_USER_PROFILE => {
            let arguments: UserProfileArguments =
                serde_json::from_value(params.arguments).map_err(|_| ())?;
            tool_text_result(
                &user_profile(pool, chat_id, arguments.telegram_user_id)
                    .await
                    .map_err(|_| ())?,
            )
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

async fn resolve_user(pool: &PgPool, chat_id: i64, arguments: Value) -> Result<Value, ()> {
    let arguments: ResolveUserArguments = serde_json::from_value(arguments).map_err(|_| ())?;
    let query = arguments
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(|query| query.chars().take(80).collect::<String>());
    if arguments.telegram_user_id.is_none() && query.is_none() {
        return Err(());
    }
    let query_variants = query.as_deref().map(resolve_query_variants);
    let mut users = sqlx::query_as::<_, ResolvedUserRow>(
        r#"
        select p.telegram_user_id, nullif(p.username, '') as username,
               coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''),
                        nullif(p.username, ''), 'Неизвестный пользователь') as display_name,
               case
                   when p.telegram_user_id = $2 then 0
                   when lower(coalesce(p.username, '')) = any(coalesce($3::text[], array[]::text[])) then 1
                   when regexp_replace(lower(concat_ws(' ', p.first_name, p.last_name)), '[^[:alnum:]_]+', '', 'g') = any(coalesce($3::text[], array[]::text[])) then 2
                   else 3
               end as match_rank,
               coalesce(cu.message_count, 0) as message_count
        from telegram_user_profiles p
        left join telegram_chat_users cu
          on cu.chat_id = $1 and cu.telegram_user_id = p.telegram_user_id
        where exists (
            select 1 from telegram_messages m
            where m.chat_id = $1 and m.user_id = p.telegram_user_id
        )
          and ($2::bigint is null or p.telegram_user_id = $2)
          and ($3::text[] is null or exists (
              select 1 from unnest($3::text[]) candidate
              where position(lower(candidate) in lower(concat_ws(' ', p.username, p.first_name, p.last_name))) > 0
                 or position(candidate in regexp_replace(lower(concat_ws(' ', p.username, p.first_name, p.last_name)), '[^[:alnum:]_]+', '', 'g')) > 0
          ))
        order by
            case when p.telegram_user_id = $2 then 0 else 1 end,
            case when lower(coalesce(p.username, '')) = any(coalesce($3::text[], array[]::text[])) then 0 else 1 end,
            case when regexp_replace(lower(concat_ws(' ', p.first_name, p.last_name)), '[^[:alnum:]_]+', '', 'g') = any(coalesce($3::text[], array[]::text[])) then 0 else 1 end,
            coalesce(cu.message_count, 0) desc,
            p.last_seen_at desc
        limit 10
        "#,
    )
    .bind(chat_id)
    .bind(arguments.telegram_user_id)
    .bind(&query_variants)
    .fetch_all(pool)
    .await
    .map_err(|_| ())?;
    if users.is_empty() && arguments.telegram_user_id.is_none() {
        users = fuzzy_resolve_users(pool, chat_id, query_variants.as_deref().unwrap_or_default())
            .await?;
    }
    let users = users
        .into_iter()
        .enumerate()
        .map(|(index, user)| {
            json!({
                "telegram_user_id": user.telegram_user_id,
                "username": user.username,
                "display_name": user.display_name,
                "author_url": public_username_url(user.username.as_deref()),
                "message_count": user.message_count,
                "match": match user.match_rank {
                    0 => "telegram_id",
                    1 => "username",
                    2 => "exact_name",
                    3 => "partial_name",
                    _ => "fuzzy_name",
                },
                "recommended": index == 0
            })
        })
        .collect::<Vec<_>>();
    tool_text_result(&users)
}

async fn fuzzy_resolve_users(
    pool: &PgPool,
    chat_id: i64,
    query_variants: &[String],
) -> Result<Vec<ResolvedUserRow>, ()> {
    if query_variants.is_empty() {
        return Ok(Vec::new());
    }
    let candidates = sqlx::query_as::<_, FuzzyUserRow>(
        r#"
        select p.telegram_user_id, nullif(p.username, '') as username,
               coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''),
                        nullif(p.username, ''), 'Неизвестный пользователь') as display_name,
               coalesce(cu.message_count, 0) as message_count
        from telegram_user_profiles p
        left join telegram_chat_users cu
          on cu.chat_id = $1 and cu.telegram_user_id = p.telegram_user_id
        where exists (
            select 1 from telegram_messages m
            where m.chat_id = $1 and m.user_id = p.telegram_user_id
        )
        order by coalesce(cu.message_count, 0) desc, p.last_seen_at desc
        limit 5000
        "#,
    )
    .bind(chat_id)
    .fetch_all(pool)
    .await
    .map_err(|_| ())?;

    let mut matches = candidates
        .into_iter()
        .filter_map(|candidate| {
            let labels = [
                candidate.display_name.as_str(),
                candidate.username.as_deref().unwrap_or(""),
            ];
            let distance = query_variants
                .iter()
                .flat_map(|query| {
                    labels
                        .iter()
                        .map(move |label| normalized_edit_distance(query, label))
                })
                .min_by(|left, right| {
                    left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
                })?;
            (distance <= 0.45).then_some((distance, candidate))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(left_distance, left), (right_distance, right)| {
        left_distance
            .partial_cmp(right_distance)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.message_count.cmp(&left.message_count))
    });
    Ok(matches
        .into_iter()
        .take(5)
        .map(|(_, candidate)| ResolvedUserRow {
            telegram_user_id: candidate.telegram_user_id,
            username: candidate.username,
            display_name: candidate.display_name,
            match_rank: 4,
            message_count: candidate.message_count,
        })
        .collect())
}

fn normalized_edit_distance(left: &str, right: &str) -> f32 {
    let left = normalize_resolve_label(left);
    let right = normalize_resolve_label(right);
    let max_len = left.chars().count().max(right.chars().count());
    if max_len == 0 {
        return 1.0;
    }
    levenshtein_chars(&left, &right) as f32 / max_len as f32
}

fn normalize_resolve_label(value: &str) -> String {
    let normalized = value
        .trim()
        .trim_start_matches('@')
        .to_lowercase()
        .chars()
        .filter(|character| character.is_alphanumeric() || *character == '_')
        .collect::<String>();
    // Фонетические варианты после русско-латинской транслитерации: «ноунейм» → nouneim.
    // Это применяется только в fallback-поиске, точное сопоставление выше остаётся неизменным.
    normalized
        .replace("ou", "o")
        .replace("eim", "ame")
        .replace("ei", "e")
        .replace("ie", "e")
}

fn levenshtein_chars(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_character) in left.chars().enumerate() {
        let mut current = vec![left_index + 1];
        for (right_index, right_character) in right.iter().enumerate() {
            let replace_cost = usize::from(left_character != *right_character);
            current.push(
                (previous[right_index + 1] + 1)
                    .min(current[right_index] + 1)
                    .min(previous[right_index] + replace_cost),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

fn resolve_query_variants(query: &str) -> Vec<String> {
    let normalized = query.trim().trim_start_matches('@').to_lowercase();
    let compact = normalized
        .chars()
        .filter(|character| character.is_alphanumeric() || *character == '_')
        .collect::<String>();
    let mut variants = vec![normalized, compact.clone()];
    if compact.chars().count() >= 4 {
        if let Some(stem) = compact.strip_suffix('и') {
            variants.extend([stem.to_string(), format!("{stem}а"), format!("{stem}я")]);
        }
    }
    let transliterated = variants
        .iter()
        .map(|variant| transliterate_russian(variant))
        .collect::<Vec<_>>();
    variants.extend(transliterated);
    variants.retain(|variant| !variant.is_empty());
    variants.sort();
    variants.dedup();
    variants
}

fn transliterate_russian(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            match character {
                'а' => "a",
                'б' => "b",
                'в' => "v",
                'г' => "g",
                'д' => "d",
                'е' | 'ё' => "e",
                'ж' => "zh",
                'з' => "z",
                'и' | 'й' => "i",
                'к' => "k",
                'л' => "l",
                'м' => "m",
                'н' => "n",
                'о' => "o",
                'п' => "p",
                'р' => "r",
                'с' => "s",
                'т' => "t",
                'у' => "u",
                'ф' => "f",
                'х' => "h",
                'ц' => "ts",
                'ч' => "ch",
                'ш' => "sh",
                'щ' => "sch",
                'ы' => "y",
                'э' => "e",
                'ю' => "yu",
                'я' => "ya",
                'ь' | 'ъ' => "",
                _ => return character.to_string(),
            }
            .to_string()
        })
        .collect()
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
        {"name": TOOL_SEARCH_MESSAGES, "description": "Ищет сообщения только в разрешённом чате. full_text ищет слова и выражения, literal — точную подстроку.", "inputSchema": {"type": "object", "additionalProperties": false, "required": ["query"], "properties": {"query": {"type": "string", "maxLength": 240}, "user_id": {"type": "integer"}, "date_from": {"type": "string", "format": "date-time"}, "date_to": {"type": "string", "format": "date-time"}, "reply_to_message_id": {"type": "integer"}, "has_links": {"type": "boolean"}, "has_media": {"type": "boolean"}, "match_mode": {"type": "string", "enum": ["full_text", "literal"]}, "sort": {"type": "string", "enum": ["relevance", "newest", "oldest"]}, "limit": {"type": "integer", "minimum": 1, "maximum": 20}}}},
        {"name": TOOL_SEARCH_MESSAGES_BATCH, "description": "Выполняет 1–6 независимых поисковых запросов с общими фильтрами. Используй для синонимов и разных формулировок вместо склеивания всех слов в один AND-запрос.", "inputSchema": {"type": "object", "additionalProperties": false, "required": ["queries"], "properties": {"queries": {"type": "array", "minItems": 1, "maxItems": 6, "items": {"type": "string", "maxLength": 240}}, "user_id": {"type": "integer"}, "date_from": {"type": "string", "format": "date-time"}, "date_to": {"type": "string", "format": "date-time"}, "has_links": {"type": "boolean"}, "has_media": {"type": "boolean"}, "match_mode": {"type": "string", "enum": ["full_text", "literal"]}, "sort": {"type": "string", "enum": ["relevance", "newest", "oldest"]}, "limit_per_query": {"type": "integer", "minimum": 1, "maximum": 5}}}},
        {"name": TOOL_RECENT_MESSAGES, "description": "Возвращает последние или первые сообщения чата без поискового запроса, с фильтрами по автору и времени.", "inputSchema": {"type": "object", "additionalProperties": false, "properties": {"user_id": {"type": "integer"}, "date_from": {"type": "string", "format": "date-time"}, "date_to": {"type": "string", "format": "date-time"}, "has_links": {"type": "boolean"}, "has_media": {"type": "boolean"}, "sort": {"type": "string", "enum": ["newest", "oldest"]}, "limit": {"type": "integer", "minimum": 1, "maximum": 20}}}},
        {"name": TOOL_GET_MESSAGE, "description": "Возвращает одно сообщение по ID в разрешённом чате.", "inputSchema": {"type": "object", "additionalProperties": false, "required": ["message_id"], "properties": {"message_id": {"type": "integer"}}}},
        {"name": TOOL_MESSAGE_CONTEXT, "description": "Возвращает ограниченный контекст вокруг найденного сообщения.", "inputSchema": {"type": "object", "additionalProperties": false, "required": ["message_id"], "properties": {"message_id": {"type": "integer"}, "before": {"type": "integer", "minimum": 0, "maximum": 5}, "after": {"type": "integer", "minimum": 0, "maximum": 5}}}}
        ,{"name": TOOL_REPLY_THREAD, "description": "Возвращает родителей и дочерние ответы ветки вокруг сообщения (до 20 сообщений).", "inputSchema": {"type": "object", "additionalProperties": false, "required": ["message_id"], "properties": {"message_id": {"type": "integer"}}}}
        ,{"name": TOOL_RESOLVE_USER, "description": "Находит участника разрешённого чата по точному Telegram ID, username или отображаемому имени. Перед вопросом о конкретном человеке сначала используй этот инструмент.", "inputSchema": {"type": "object", "additionalProperties": false, "properties": {"query": {"type": "string", "maxLength": 80}, "telegram_user_id": {"type": "integer"}}}}
        ,{"name": TOOL_USER_INTERACTIONS, "description": "Возвращает последние прямые reply-взаимодействия между двумя участниками разрешённого чата. Для каждого reply включает и ответ, и сообщение, на которое отвечали.", "inputSchema": {"type": "object", "additionalProperties": false, "required": ["first_user_id", "second_user_id"], "properties": {"first_user_id": {"type": "integer"}, "second_user_id": {"type": "integer"}, "limit": {"type": "integer", "minimum": 1, "maximum": 20}}}}
        ,{"name": TOOL_USER_PROFILE, "description": "Возвращает безопасный профиль и агрегированную активность участника разрешённого чата.", "inputSchema": {"type": "object", "additionalProperties": false, "required": ["telegram_user_id"], "properties": {"telegram_user_id": {"type": "integer"}}}}
        ,{"name": TOOL_LIST_CHAT_NOTES, "description": "Возвращает активные общие заметки разрешённого чата.", "inputSchema": {"type": "object", "additionalProperties": false, "properties": {}}}
        ,{"name": TOOL_LIST_USER_NOTES, "description": "Возвращает активные заметки указанного пользователя в разрешённом чате.", "inputSchema": {"type": "object", "additionalProperties": false, "required": ["telegram_user_id"], "properties": {"telegram_user_id": {"type": "integer"}}}}
    ]})
}

fn tool_text_result<T: Serialize>(value: &T) -> Result<Value, ()> {
    let text = serde_json::to_string(value).map_err(|_| ())?;
    Ok(json!({"content": [{"type": "text", "text": text}], "isError": false}))
}

fn public_username_url(username: Option<&str>) -> Option<String> {
    let username = username?.trim();
    ((5..=32).contains(&username.len())
        && username
            .bytes()
            .all(|character| character.is_ascii_alphanumeric() || character == b'_'))
    .then(|| format!("https://t.me/{username}"))
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
                TOOL_SEARCH_MESSAGES_BATCH,
                TOOL_RECENT_MESSAGES,
                TOOL_GET_MESSAGE,
                TOOL_MESSAGE_CONTEXT,
                TOOL_REPLY_THREAD,
                TOOL_RESOLVE_USER,
                TOOL_USER_INTERACTIONS,
                TOOL_USER_PROFILE,
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

    #[test]
    fn fuzzy_resolution_accepts_transliterated_display_name() {
        let variants = resolve_query_variants("ноунейм");
        let best_no_name = variants
            .iter()
            .map(|variant| normalized_edit_distance(variant, "NoName"))
            .min_by(|left, right| left.partial_cmp(right).unwrap())
            .unwrap();
        let best_none = variants
            .iter()
            .map(|variant| normalized_edit_distance(variant, "None"))
            .min_by(|left, right| left.partial_cmp(right).unwrap())
            .unwrap();
        assert!(best_no_name <= 0.45);
        assert!(best_no_name < best_none, "{best_no_name} >= {best_none}");
    }

    #[test]
    fn fuzzy_resolution_rejects_unrelated_short_name() {
        assert!(normalized_edit_distance("паша", "NoName") > 0.45);
    }
}
