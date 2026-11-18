use super::*;

#[derive(Default)]
pub(super) struct EvidenceTracker {
    pub(super) message_ids: Vec<i32>,
    pub(super) message_ids_by_user: HashMap<i64, Vec<i32>>,
    pub(super) source_urls: Vec<String>,
    pub(super) searched_scopes: HashSet<String>,
    pub(super) verified_counts: Vec<Value>,
}

#[derive(Default)]
pub(super) struct ScopeTracker {
    pub(super) message_ids: HashSet<i32>,
    pub(super) strict: bool,
}

pub(super) fn count_scope_verified(arguments: &Value, evidence: &EvidenceTracker) -> bool {
    arguments.is_object()
        && (arguments.get("query").is_none_or(|query| {
            query.is_null() || query.as_str().is_some_and(|text| text.trim().is_empty())
        }) || scope_signature(arguments)
            .is_some_and(|scope| evidence.searched_scopes.contains(&scope)))
}

pub(super) fn requests_chat_wide_scope(question: &str) -> bool {
    let question = question.to_lowercase();
    let broad_scope_phrases = [
        "по всему чату",
        "во всём чате",
        "во всем чате",
        "по всей истории",
        "за всю историю",
        "за всю переписку",
        "за всё время",
        "за все время",
        "в целом по чату",
        "по всему каналу",
        "ищи по другим веткам",
        "поищи по другим веткам",
        "проверь другие ветки",
        "сравни сообщения в разных ветках",
        "сравни разные ветки",
        "что обсуждали в разных ветках",
        "что было в разных ветках",
        "across other threads",
        "whole chat",
        "entire chat",
        "across the chat",
        "all history",
    ];
    broad_scope_phrases.iter().any(|phrase| {
        question.match_indices(phrase).any(|(start, _)| {
            let before_phrase = question[..start]
                .rsplit(['.', '!', '?', ';'])
                .next()
                .unwrap_or_default();
            !before_phrase
                .split_whitespace()
                .rev()
                .take(5)
                .any(|word| matches!(word, "не" | "ни" | "без" | "not" | "never" | "don't"))
        })
    })
}

pub(super) fn reply_scope_blocks_global_count(reply_scope: &ScopeTracker, tool: &str) -> bool {
    reply_scope.strict && matches!(tool, "chat.count_messages" | "chat.count_word_occurrences")
}

pub(super) fn reply_ancestor_context(
    value: &Value,
    target_message_id: i32,
) -> (Value, HashSet<i32>) {
    let Some(messages) = value.get("thread").and_then(Value::as_array) else {
        return (
            json!({"root_message_id": target_message_id, "thread": []}),
            HashSet::new(),
        );
    };
    let by_id = messages
        .iter()
        .filter_map(|message| {
            let id = message
                .get("message_id")
                .and_then(Value::as_i64)
                .and_then(|id| i32::try_from(id).ok())?;
            Some((id, message))
        })
        .collect::<HashMap<_, _>>();
    let mut path = Vec::new();
    let mut message_ids = HashSet::new();
    let mut current_id = target_message_id;
    while message_ids.insert(current_id) {
        let Some(message) = by_id.get(&current_id) else {
            message_ids.remove(&current_id);
            break;
        };
        path.push((*message).clone());
        let Some(parent_id) = message
            .get("reply_to_message_id")
            .and_then(Value::as_i64)
            .and_then(|id| i32::try_from(id).ok())
        else {
            break;
        };
        current_id = parent_id;
    }
    path.reverse();
    let root_message_id = path
        .first()
        .and_then(|message| message.get("message_id"))
        .cloned()
        .unwrap_or_else(|| json!(target_message_id));
    (
        json!({"root_message_id": root_message_id, "thread": path}),
        message_ids,
    )
}

pub(super) fn reply_memory_root_id(reply_history: &Value, target_message_id: i32) -> Option<i32> {
    let path = reply_history.get("thread")?.as_array()?;
    let ids = path
        .iter()
        .filter_map(|message| {
            message
                .get("message_id")
                .and_then(Value::as_i64)
                .and_then(|id| i32::try_from(id).ok())
        })
        .collect::<Vec<_>>();
    if ids.last().copied() != Some(target_message_id) {
        return None;
    }
    ids.first().copied()
}

pub(super) fn reply_thread_prefetch_allowed(profile: &AgentProfile) -> bool {
    profile
        .tool_allowlist
        .iter()
        .any(|tool| tool == "chat.get_reply_thread")
}

pub(super) fn filter_reply_scoped_result(
    tool: &str,
    value: &Value,
    allowed_ids: &HashSet<i32>,
) -> Value {
    let mut filtered = value.clone();
    match tool {
        "chat.search_messages" | "chat.get_recent_messages" => {
            filter_message_page(&mut filtered, "messages", allowed_ids);
        }
        "voice.list_transcripts" => {
            filter_message_page(&mut filtered, "rows", allowed_ids);
            filtered["reply_scope_filter_applied"] = json!(true);
        }
        "memory.list_notes" => {
            return json!({"rows":[],"reply_scope_filter_applied":true,"note":"Общие RAG-карточки вне цепочки ответа не подгружаются без запроса по всему чату."});
        }
        "chat.search_messages_batch" => {
            if let Some(results) = filtered.get_mut("results").and_then(Value::as_array_mut) {
                for result in results {
                    filter_message_page(result, "messages", allowed_ids);
                }
            }
        }
        "chat.get_message_context" => {
            filter_message_array(&mut filtered, "context", allowed_ids);
        }
        "chat.get_reply_thread" => {
            filter_message_array(&mut filtered, "thread", allowed_ids);
        }
        "chat.get_message" => {
            if let Some(object) = filtered.as_object_mut()
                && object
                    .get("message")
                    .is_some_and(|message| !message_is_allowed(message, allowed_ids))
            {
                object.insert("found".into(), json!(false));
                object.insert("message".into(), Value::Null);
            }
        }
        "chat.get_user_interactions" => {
            if let Some(interactions) = filtered
                .get_mut("interactions")
                .and_then(Value::as_array_mut)
            {
                interactions.retain_mut(|interaction| {
                    let Some(object) = interaction.as_object_mut() else {
                        return false;
                    };
                    let Some(message) = object.get("message") else {
                        return false;
                    };
                    if !message_is_allowed(message, allowed_ids) {
                        return false;
                    }
                    if object
                        .get("replied_to")
                        .is_some_and(|message| !message_is_allowed(message, allowed_ids))
                    {
                        object.insert("replied_to".into(), Value::Null);
                    }
                    true
                });
            }
        }
        _ => {}
    }
    if matches!(
        tool,
        "chat.search_messages"
            | "chat.search_messages_batch"
            | "chat.get_recent_messages"
            | "chat.get_message_context"
            | "chat.get_reply_thread"
            | "chat.get_message"
            | "chat.get_user_interactions"
    ) && let Some(object) = filtered.as_object_mut()
    {
        object.insert("reply_scope_filter_applied".into(), json!(true));
    }
    filtered
}

pub(super) fn filter_message_page(value: &mut Value, field: &str, allowed_ids: &HashSet<i32>) {
    filter_message_array(value, field, allowed_ids);
    if let Some(object) = value.as_object_mut()
        && object.get(field).is_some_and(Value::is_array)
    {
        object.insert("total_count".into(), Value::Null);
        object.insert("has_more".into(), Value::Null);
        object.insert("next_offset".into(), Value::Null);
        object.insert("scan_limit_reached".into(), Value::Null);
        object.insert("reply_scope_filter_applied".into(), json!(true));
    }
}

pub(super) fn filter_message_array(value: &mut Value, field: &str, allowed_ids: &HashSet<i32>) {
    if let Some(messages) = value.get_mut(field).and_then(Value::as_array_mut) {
        messages.retain(|message| message_is_allowed(message, allowed_ids));
    }
}

pub(super) fn message_is_allowed(message: &Value, allowed_ids: &HashSet<i32>) -> bool {
    message
        .get("message_id")
        .and_then(Value::as_i64)
        .and_then(|id| i32::try_from(id).ok())
        .is_some_and(|id| allowed_ids.contains(&id))
}

pub(super) fn record_search_scopes(
    tool: &str,
    arguments: &Value,
    result: &Value,
    evidence: &mut EvidenceTracker,
) {
    if tool == "chat.search_messages" {
        if let Some(scope) = scope_signature(arguments) {
            evidence.searched_scopes.insert(scope);
        }
    } else if tool == "chat.search_messages_batch" {
        for item in result
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let mut scope = arguments.clone();
            if let Some(object) = scope.as_object_mut() {
                object.remove("queries");
                object.remove("limit_per_query");
                object.insert("query".into(), item["query"].clone());
                record_search_scopes("chat.search_messages", &scope, item, evidence);
            }
        }
    }
}

pub(super) fn collect_message_evidence_value(value: &Value, evidence: &mut EvidenceTracker) {
    if let Some(item) = value.as_object()
        && let Some(message_id) = item
            .get("message_id")
            .and_then(Value::as_i64)
            .and_then(|id| i32::try_from(id).ok())
    {
        if !evidence.message_ids.contains(&message_id) {
            evidence.message_ids.push(message_id);
        }
        if let Some(user_id) = item.get("user_id").and_then(Value::as_i64) {
            let ids = evidence.message_ids_by_user.entry(user_id).or_default();
            if !ids.contains(&message_id) {
                ids.push(message_id);
            }
        }
    }
    match value {
        Value::Array(items) => {
            for item in items {
                collect_message_evidence_value(item, evidence);
            }
        }
        Value::Object(object) => {
            for nested in object.values() {
                collect_message_evidence_value(nested, evidence);
            }
        }
        _ => {}
    }
}

pub(super) fn collect_source_evidence_value(value: &Value, evidence: &mut EvidenceTracker) {
    collect_url_field_evidence(value, "url", evidence);
}

pub(super) fn collect_url_field_evidence(
    value: &Value,
    field: &str,
    evidence: &mut EvidenceTracker,
) {
    if let Some(url) = value
        .as_object()
        .and_then(|object| object.get(field))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        && !evidence.source_urls.iter().any(|known| known == url)
    {
        evidence.source_urls.push(url.to_owned());
    }
    match value {
        Value::Array(items) => {
            for item in items {
                collect_url_field_evidence(item, field, evidence);
            }
        }
        Value::Object(object) => {
            for nested in object.values() {
                collect_url_field_evidence(nested, field, evidence);
            }
        }
        _ => {}
    }
}

pub(super) fn push_observation(observations: &mut Vec<String>, observation: String) {
    observations.push(truncate_chars(&observation, MAX_OBSERVATION_CHARS));
    while observations
        .iter()
        .map(|value| value.chars().count())
        .sum::<usize>()
        > MAX_CONTEXT_CHARS
    {
        observations.remove(0);
    }
    log_observation_pressure(observations);
}

/// Бюджет окна наблюдений в токенах: те же 48k символов ≈ 12k токенов.
/// Только для наблюдаемости, вытеснением по-прежнему управляет лимит в символах.
pub(super) fn observation_budget() -> ContextBudget {
    ContextBudget {
        hard_input_limit: 15_000,
        target_input_limit: 12_000,
        output_reserve: 500,
        tool_reserve: 500,
        safety_margin: 500,
    }
}

pub(super) fn log_observation_pressure(observations: &[String]) {
    let tokenizer = ApproxTokenizer;
    let used = observations
        .iter()
        .map(|observation| tokenizer.count_text(observation))
        .fold(0, u32::saturating_add);
    let budget = observation_budget();
    let state = pressure(used, &budget, None);
    tracing::debug!(used_tokens = used, ?state, "ask observation pressure");
}

pub(super) fn first_chars(value: &str, limit: usize) -> String {
    truncate_chars(value, limit)
}
