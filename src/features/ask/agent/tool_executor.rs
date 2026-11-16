use super::*;

#[derive(Clone, Default)]
pub(super) struct ToolResult {
    pub(super) value: Value,
    pub(super) agent_preview: String,
}

pub(super) struct ToolCallContext<'a> {
    pub(super) config: &'a Config,
    pub(super) pool: &'a PgPool,
    pub(super) scope_chat_id: i64,
    pub(super) conversation_chat_id: i64,
    pub(super) requester_user_id: i64,
    pub(super) command_message_id: i32,
    pub(super) question: &'a str,
    pub(super) evidence: &'a mut EvidenceTracker,
    pub(super) reply_scope: &'a ScopeTracker,
    pub(super) mcp: &'a McpClient,
    pub(super) python_sandbox: Option<&'a mut AskPythonSandbox>,
    pub(super) allow_mutations: bool,
    pub(super) memory_write_policy: crate::llm::profiles::AgentMemoryWritePolicy,
    pub(super) explicit_memory_write_requested: bool,
}

pub(super) fn should_cache_tool_result(tool: &str) -> bool {
    !matches!(
        tool,
        "chat.count_messages" | "chat.count_word_occurrences" | "sandbox.python"
    )
}

pub(super) fn cached_tool_result<'a>(
    tool: &str,
    signature: &str,
    cache: &'a HashMap<String, ToolResult>,
) -> Option<&'a ToolResult> {
    should_cache_tool_result(tool)
        .then(|| cache.get(signature))
        .flatten()
}

pub(super) fn durable_note_write_denial(
    allow_mutations: bool,
    policy: crate::llm::profiles::AgentMemoryWritePolicy,
    explicit_request: bool,
) -> Option<&'static str> {
    if !allow_mutations {
        Some("диагностический replay не сохраняет заметки")
    } else if policy == crate::llm::profiles::AgentMemoryWritePolicy::Disabled {
        Some("долговременная запись заметок отключена профилем")
    } else if !explicit_request {
        Some("для долговременной заметки нужна явная просьба пользователя запомнить")
    } else {
        None
    }
}

pub(super) fn audit_tool_arguments(tool: &str, arguments: &Value) -> Value {
    if tool == "sandbox.python" {
        let code_bytes = arguments
            .get("code")
            .and_then(Value::as_str)
            .map_or(0, str::len);
        return json!({"code_redacted": true, "code_bytes": code_bytes});
    }
    arguments.clone()
}

pub(super) fn local_agent_tools() -> Vec<Tool> {
    vec![
        Tool::new(wire_tool_name("notes.add_user"))
            .with_description("Сохранить короткий подтверждённый факт о пользователе.")
            .with_schema(json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["telegram_user_id", "note"],
                "properties": {
                    "telegram_user_id": {"type": "integer"},
                    "note": {"type": "string"}
                }
            }))
            .with_strict(true),
        Tool::new(wire_tool_name("web.search"))
            .with_description("Найти актуальные внешние факты и прочитать результаты поиска.")
            .with_schema(json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["query"],
                "properties": {"query": {"type": "string"}}
            }))
            .with_strict(true),
        Tool::new(wire_tool_name("github.search"))
            .with_description("Найти публичный код, issue или репозиторий на GitHub.")
            .with_schema(json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["query"],
                "properties": {"query": {"type": "string"}}
            }))
            .with_strict(true),
    ]
}

pub(super) fn profile_agent_tools(
    profile: &AgentProfile,
    mcp: &McpClient,
    web_search_enabled: bool,
    github_search_enabled: bool,
) -> Vec<Tool> {
    let mut tools = mcp
        .genai_tools()
        .iter()
        .filter(|tool| {
            mcp.canonical_tool_name(tool.name.as_ref())
                .is_some_and(|canonical| {
                    profile_allows_tool(
                        profile,
                        canonical,
                        web_search_enabled,
                        github_search_enabled,
                    )
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    tools.extend(local_agent_tools().into_iter().filter(|tool| {
        let canonical = RESERVED_LOCAL_AGENT_TOOLS
            .iter()
            .find(|name| wire_tool_name(name) == tool.name.as_ref())
            .copied();
        canonical.is_some_and(|name| {
            profile_allows_tool(profile, name, web_search_enabled, github_search_enabled)
        })
    }));
    tools
}

pub(super) fn profile_allows_tool(
    profile: &AgentProfile,
    canonical_name: &str,
    web_search_enabled: bool,
    github_search_enabled: bool,
) -> bool {
    if !profile
        .tool_allowlist
        .iter()
        .any(|allowed| allowed == canonical_name)
    {
        return false;
    }
    match canonical_name {
        "notes.add_user" => {
            profile.memory.write_policy != crate::llm::profiles::AgentMemoryWritePolicy::Disabled
        }
        "web.search" => web_search_enabled,
        "github.search" => github_search_enabled,
        _ => true,
    }
}

pub(super) fn canonical_native_tool(
    mcp: &McpClient,
    tools: &[Tool],
    wire_tool: &str,
) -> Option<String> {
    if !tools
        .iter()
        .any(|candidate| candidate.name.to_string() == wire_tool)
    {
        return None;
    }
    mcp.canonical_tool_name(wire_tool)
        .map(str::to_owned)
        .or_else(|| {
            RESERVED_LOCAL_AGENT_TOOLS
                .iter()
                .find(|canonical| wire_tool_name(canonical) == wire_tool)
                .map(|canonical| (*canonical).to_string())
        })
}

pub(super) async fn audit_tool_call(
    pool: &PgPool,
    ask_run_id: Option<i64>,
    pending: PendingToolCallAudit<'_>,
) {
    let Some(ask_run_id) = ask_run_id else {
        return;
    };
    let tool_name = pending.tool_name();
    if let Err(err) = repo::record_tool_call(pool, pending.into_audit(ask_run_id)).await {
        tracing::warn!(%err, ask_run_id, tool_name, "failed to audit ask tool call");
    }
}

pub(super) fn elapsed_millis(started: Instant) -> Option<i64> {
    i64::try_from(started.elapsed().as_millis()).ok()
}

pub(super) fn tool_result_count(value: &Value) -> Option<i64> {
    if let Some(count) = value.get("count").and_then(Value::as_i64) {
        return Some(count);
    }
    let count = match value {
        Value::Array(items) => items.len(),
        Value::Object(object) => ["messages", "results", "context", "thread", "interactions"]
            .iter()
            .find_map(|field| object.get(*field).and_then(Value::as_array).map(Vec::len))?,
        _ => return None,
    };
    i64::try_from(count).ok()
}

pub(super) fn scope_signature(arguments: &Value) -> Option<String> {
    let mut value = arguments.clone();
    let object = value.as_object_mut()?;
    for field in ["sort", "limit", "offset"] {
        object.remove(field);
    }
    object.retain(|_, value| !value.is_null());
    let query = object
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    object.insert("query".into(), Value::String(query));
    object.entry("match_mode").or_insert(json!("hybrid"));
    object.entry("include_forwards").or_insert(json!(false));
    for field in ["date_from", "date_to"] {
        if let Some(timestamp) = object
            .get(field)
            .and_then(Value::as_str)
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        {
            object.insert(
                field.into(),
                json!(timestamp.with_timezone(&Utc).to_rfc3339()),
            );
        }
    }
    let ordered = object.iter().collect::<std::collections::BTreeMap<_, _>>();
    serde_json::to_string(&ordered).ok()
}

pub(super) fn agent_tool_preview(
    tool: &str,
    arguments: &Value,
    result: &ToolResult,
    limit: usize,
) -> String {
    let value_limit = limit
        .saturating_sub(MAX_TOOL_ARGUMENTS_PREVIEW_CHARS)
        .max(256);
    let preview_value = if tool == "youtube.get_subtitles" {
        subtitle_preview(&result.value, value_limit)
    } else {
        result.value.clone()
    };
    let value = structured_preview(&preview_value, value_limit)
        .ok()
        .and_then(|preview| serde_json::from_str::<Value>(&preview).ok())
        .unwrap_or_else(|| json!({"error":"tool preview unavailable"}));
    let arguments = audit_tool_arguments(tool, arguments);
    let arguments = structured_preview(&arguments, MAX_TOOL_ARGUMENTS_PREVIEW_CHARS)
        .ok()
        .and_then(|preview| serde_json::from_str::<Value>(&preview).ok())
        .unwrap_or_else(|| json!({"error":"tool arguments omitted"}));
    serialize_prompt_json(&json!({
        "kind": "untrusted_tool_result",
        "tool": tool,
        "arguments": arguments,
        "result": value,
    }))
}

/// При обрезке для модели курсор указывает на реально показанный текст,
/// а не на конец исходной MCP-страницы: иначе часть ролика терялась бы навсегда.
pub(super) fn subtitle_preview(value: &Value, limit: usize) -> Value {
    let mut preview = value.clone();
    while serialize_prompt_json(&preview).chars().count() > limit {
        let Some(videos) = preview.get_mut("videos").and_then(Value::as_array_mut) else {
            break;
        };
        let mut changed = false;
        for video in videos {
            let Some(text) = video.get("transcript").and_then(Value::as_str) else {
                continue;
            };
            let count = text.chars().count();
            if count == 0 {
                continue;
            }
            let text = truncate_chars(text, count / 2);
            let shown = text.chars().count();
            let offset = video["offset"].as_u64().unwrap_or(0);
            video["transcript"] = json!(text);
            video["returned_chars"] = json!(shown);
            video["has_more"] = json!(true);
            video["next_offset"] = json!(offset + shown as u64);
            video["note"] = json!(
                "Страница сокращена для контекста. Продолжите с next_offset и transcript_id."
            );
            changed = true;
        }
        if !changed {
            return json!({"error":"Недостаточно места для страницы субтитров. Запросите один URL отдельным вызовом."});
        }
    }
    preview
}

pub(super) async fn call_tool(
    context: ToolCallContext<'_>,
    tool: &str,
    arguments: Value,
) -> anyhow::Result<ToolResult> {
    match tool {
        tool if context.mcp.has_tool(tool) => {
            let result = context.mcp.call(tool, arguments).await.map_err(|error| {
                let kind = if AskFailureKind::from_error(&error) == AskFailureKind::Timeout {
                    AskFailureKind::Timeout
                } else {
                    AskFailureKind::ToolError
                };
                AskFailureTag::wrap(kind, error)
            })?;
            let value = if context.reply_scope.strict {
                filter_reply_scoped_result(tool, &result.value, &context.reply_scope.message_ids)
            } else {
                result.value
            };
            collect_message_evidence_value(&value, context.evidence);
            collect_url_field_evidence(&value, "author_url", context.evidence);
            if tool == "youtube.get_subtitles" {
                collect_source_evidence_value(&value, context.evidence);
            }
            if tool == "memory.list_notes" {
                collect_url_field_evidence(&value, "external_source_url", context.evidence);
            }
            let preview = if tool == "youtube.get_subtitles" {
                subtitle_preview(&value, MAX_TOOL_PREVIEW_CHARS)
            } else {
                value.clone()
            };
            let agent_preview = structured_preview(&preview, MAX_TOOL_PREVIEW_CHARS)?;
            Ok(ToolResult {
                value,
                agent_preview,
            })
        }
        "notes.add_user" => {
            let note_scope = AgentNoteScope {
                conversation_chat_id: context.conversation_chat_id,
                retrieval_scope_chat_id: context.scope_chat_id,
            };
            if note_scope.shared_chat_id().is_err() {
                return ToolResult::from_value(json!({
                    "saved": false,
                    "reason": "Личный диалог не может записывать факты в общие заметки чата."
                }));
            }
            if let Some(reason) = durable_note_write_denial(
                context.allow_mutations,
                context.memory_write_policy,
                context.explicit_memory_write_requested,
            ) {
                return ToolResult::from_value(json!({
                    "saved": false,
                    "dry_run": !context.allow_mutations,
                    "reason": reason,
                }));
            }
            let user_id = arguments
                .get("telegram_user_id")
                .and_then(Value::as_i64)
                .ok_or_else(|| anyhow::anyhow!("notes.add_user requires telegram_user_id"))?;
            if user_id != context.requester_user_id {
                return ToolResult::from_value(json!({
                    "saved": false,
                    "reason": "долговременная память разрешена только для фактов, явно сообщённых самим пользователем"
                }));
            }
            let note = arguments
                .get("note")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("notes.add_user requires note"))?;
            if !crate::features::ask::notes::explicit_note_is_safe(context.question, note) {
                return ToolResult::from_value(json!({
                    "saved": false,
                    "reason": "заметка должна быть безопасным фактом из явной просьбы самого пользователя"
                }));
            }
            let source_message_ids = context
                .evidence
                .message_ids_by_user
                .get(&user_id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if source_message_ids.is_empty() {
                add_user_note_from_explicit_request(
                    context.pool,
                    &note_scope,
                    user_id,
                    context.requester_user_id,
                    note,
                    context.question,
                    context.command_message_id,
                )
                .await?;
            } else {
                add_user_note_from_search(
                    context.pool,
                    &note_scope,
                    user_id,
                    context.requester_user_id,
                    note,
                    source_message_ids,
                )
                .await?;
            }
            ToolResult::from_value(json!({"saved": true}))
        }
        "sandbox.python" => {
            let sandbox = context
                .python_sandbox
                .ok_or_else(|| anyhow::anyhow!("Python sandbox is disabled"))?;
            let code = arguments
                .get("code")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("sandbox.python requires code"))?;
            ToolResult::from_value(sandbox.execute(code).await?)
        }
        "web.search" => {
            let result = external_search(context.config, SearchSource::Web, arguments).await?;
            collect_source_evidence_value(&result.value, context.evidence);
            Ok(result)
        }
        "github.search" => {
            let result = external_search(context.config, SearchSource::Github, arguments).await?;
            collect_source_evidence_value(&result.value, context.evidence);
            Ok(result)
        }
        _ => anyhow::bail!("ask agent requested a forbidden tool"),
    }
}

pub(super) async fn external_search(
    config: &Config,
    source: SearchSource,
    arguments: Value,
) -> anyhow::Result<ToolResult> {
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .ok_or_else(|| anyhow::anyhow!("external search requires query"))?;
    ToolResult::from_value(serde_json::to_value(
        search_for_ask(config, source, query).await?,
    )?)
}

impl ToolResult {
    pub(super) fn from_value(value: Value) -> anyhow::Result<Self> {
        Ok(Self {
            agent_preview: structured_preview(&value, MAX_TOOL_PREVIEW_CHARS)?,
            value,
        })
    }
}

/// Executes one already-authorized native tool call and records its evidence.
pub(super) struct ToolCallExecutor;

impl ToolCallExecutor {
    pub(super) async fn execute(
        context: ToolCallContext<'_>,
        tool: &str,
        arguments: Value,
    ) -> anyhow::Result<ToolResult> {
        call_tool(context, tool, arguments).await
    }
}
