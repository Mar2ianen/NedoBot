use crate::{ProviderError, ResponsesFailure, Result};
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

const MAX_SSE_EVENT_BYTES: usize = 1024 * 1024;

/// Ограниченный набор параметров публичного Responses API для ChatGPT plan usage.
/// `store=false` и `stream=true` добавляет библиотека; поля `previous_response_id`,
/// `temperature` и `max_output_tokens` намеренно недоступны.
#[derive(Clone)]
pub struct ResponsesRequest {
    model: String,
    instructions: Option<String>,
    input: Value,
    tools: Option<Vec<Value>>,
    tool_choice: Option<Value>,
}

impl ResponsesRequest {
    pub fn new(model: impl Into<String>, input: Value) -> Self {
        Self {
            model: model.into(),
            instructions: None,
            input,
            tools: None,
            tool_choice: None,
        }
    }

    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    pub fn with_tools(mut self, tools: Vec<Value>) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn with_tool_choice(mut self, tool_choice: Value) -> Self {
        self.tool_choice = Some(tool_choice);
        self
    }

    pub(crate) fn payload(&self) -> Result<Value> {
        if self.model.trim().is_empty()
            || !(self.input.is_string() || self.input.is_array())
            || self.input.as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.get("role").and_then(Value::as_str) == Some("system"))
            })
        {
            return Err(ProviderError::InvalidConfiguration);
        }

        let mut payload = json!({
            "model": &self.model,
            "input": &self.input,
            "store": false,
            "stream": true,
        });
        if let Some(instructions) = self.instructions.as_deref() {
            payload["instructions"] = Value::String(instructions.to_owned());
        }
        if let Some(tools) = self.tools.as_ref() {
            payload["tools"] = Value::Array(tools.clone());
        }
        if let Some(tool_choice) = self.tool_choice.as_ref() {
            payload["tool_choice"] = tool_choice.clone();
        }
        Ok(payload)
    }
}

impl fmt::Debug for ResponsesRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponsesRequest")
            .field("model", &self.model)
            .field(
                "instructions",
                &self.instructions.as_ref().map(|_| "[REDACTED]"),
            )
            .field("input", &"[REDACTED]")
            .field("tool_count", &self.tools.as_ref().map(Vec::len))
            .field(
                "tool_choice",
                &self.tool_choice.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct ResponsesEvent {
    pub event_type: String,
    pub data: Value,
}

impl ResponsesEvent {
    pub fn is_completed(&self) -> bool {
        self.event_type == "response.completed"
    }

    pub fn failure(&self) -> Option<ResponsesFailure> {
        if self.event_type != "response.failed" && self.event_type != "error" {
            return None;
        }
        let response = self.data.get("response");
        let error = response
            .and_then(|response| response.get("error"))
            .or_else(|| self.data.get("error"))
            .unwrap_or(&self.data);
        Some(ResponsesFailure {
            status: response
                .and_then(|response| response.get("status_code"))
                .and_then(Value::as_u64)
                .and_then(|status| u16::try_from(status).ok())
                .unwrap_or(200),
            code: error
                .get("code")
                .and_then(Value::as_str)
                .map(safe_identifier),
            param: error
                .get("param")
                .and_then(Value::as_str)
                .map(safe_identifier),
            request_id: self
                .data
                .get("request_id")
                .and_then(Value::as_str)
                .map(safe_identifier),
        })
    }
}

impl fmt::Debug for ResponsesEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponsesEvent")
            .field("event_type", &self.event_type)
            .field("data", &"[REDACTED]")
            .finish()
    }
}

pub struct ResponsesStream {
    inner: Pin<Box<dyn Stream<Item = Result<ResponsesEvent>> + Send>>,
}

impl ResponsesStream {
    pub(crate) fn from_response(response: reqwest::Response) -> Result<Self> {
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !content_type
            .split(';')
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
        {
            return Err(ProviderError::InvalidResponse);
        }

        let stream = async_stream::try_stream! {
            let mut bytes_stream = response.bytes_stream();
            let mut buffer = Vec::new();
            let mut completed = false;

            while let Some(chunk) = bytes_stream.next().await {
                let chunk = chunk.map_err(crate::error::reqwest_error)?;
                buffer.extend_from_slice(&chunk);
                while let Some((frame_end, separator_len)) = find_frame_separator(&buffer) {
                    let frame = buffer.drain(..frame_end).collect::<Vec<_>>();
                    buffer.drain(..separator_len);
                    if let Some(event) = parse_sse_frame(&frame)? {
                        if event.event_type == "response.failed" || event.event_type == "error" {
                            let failure = event.failure().ok_or(ProviderError::InvalidResponse)?;
                            Err(ProviderError::ResponsesRejected(failure))?;
                        }
                        if event.event_type == "response.incomplete" {
                            Err(ProviderError::IncompleteResponse)?;
                        }
                        completed = event.is_completed();
                        let is_terminal = completed;
                        yield event;
                        if is_terminal {
                            break;
                        }
                    }
                    if completed {
                        break;
                    }
                }
                if completed {
                    break;
                }
                if buffer.len() > MAX_SSE_EVENT_BYTES {
                    Err(ProviderError::InvalidResponse)?;
                }
            }

            if !completed && !buffer.is_empty() {
                if buffer.len() > MAX_SSE_EVENT_BYTES {
                    Err(ProviderError::InvalidResponse)?;
                }
                if let Some(event) = parse_sse_frame(&buffer)? {
                    if event.event_type == "response.failed" || event.event_type == "error" {
                        let failure = event.failure().ok_or(ProviderError::InvalidResponse)?;
                        Err(ProviderError::ResponsesRejected(failure))?;
                    }
                    if event.event_type == "response.incomplete" {
                        Err(ProviderError::IncompleteResponse)?;
                    }
                    completed = event.is_completed();
                    yield event;
                }
            }
            if !completed {
                Err(ProviderError::IncompleteResponse)?;
            }
        };

        Ok(Self {
            inner: Box::pin(stream),
        })
    }

    pub async fn next_event(&mut self) -> Option<Result<ResponsesEvent>> {
        self.inner.next().await
    }
}

impl Stream for ResponsesStream {
    type Item = Result<ResponsesEvent>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(context)
    }
}

fn find_frame_separator(bytes: &[u8]) -> Option<(usize, usize)> {
    [
        bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| (position, 4)),
        bytes
            .windows(2)
            .position(|window| window == b"\n\n")
            .map(|position| (position, 2)),
        bytes
            .windows(2)
            .position(|window| window == b"\r\r")
            .map(|position| (position, 2)),
    ]
    .into_iter()
    .flatten()
    .min_by_key(|(position, _)| *position)
}

fn parse_sse_frame(frame: &[u8]) -> Result<Option<ResponsesEvent>> {
    if frame.len() > MAX_SSE_EVENT_BYTES {
        return Err(ProviderError::InvalidResponse);
    }
    let frame = std::str::from_utf8(frame).map_err(|_| ProviderError::InvalidResponse)?;
    let mut event_type = "message".to_owned();
    let mut data = Vec::new();
    for line in frame.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => event_type = value.to_owned(),
            "data" => data.push(value),
            _ => {}
        }
    }
    if data.is_empty() {
        return Ok(None);
    }
    let data = data.join("\n");
    if data == "[DONE]" {
        return Ok(None);
    }
    let data = serde_json::from_str(&data).map_err(|_| ProviderError::InvalidResponse)?;
    Ok(Some(ResponsesEvent { event_type, data }))
}

fn safe_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '_' || *character == '-'
        })
        .take(80)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn mock_response(status: &str, body: &str) -> reqwest::Response {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_owned();
        let body = body.to_owned();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = socket.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
            }
            let headers = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
            socket.write_all(body.as_bytes()).await.unwrap();
        });
        let response = reqwest::Client::new()
            .get(format!("http://{address}/responses"))
            .send()
            .await
            .unwrap();
        server.await.unwrap();
        response
    }

    #[test]
    fn request_always_disables_storage_and_streams_without_unsupported_limits() {
        let request = ResponsesRequest::new("gpt-5-codex", json!([{"role":"user","content":"hi"}]))
            .with_instructions("help")
            .with_tools(vec![json!({"type":"function","name":"lookup"})]);
        let payload = request.payload().unwrap();

        assert_eq!(payload["store"], false);
        assert_eq!(payload["stream"], true);
        assert_eq!(payload["instructions"], "help");
        assert!(payload.get("temperature").is_none());
        assert!(payload.get("max_output_tokens").is_none());
        assert!(payload.get("previous_response_id").is_none());
        assert!(!format!("{request:?}").contains("help"));
        assert!(!format!("{request:?}").contains("hi"));
    }

    #[test]
    fn request_rejects_inline_system_messages_and_invalid_inputs() {
        assert!(
            ResponsesRequest::new("gpt-5-codex", json!([{"role":"system","content":"x"}]))
                .payload()
                .is_err()
        );
        assert!(ResponsesRequest::new("", json!("text")).payload().is_err());
        assert!(
            ResponsesRequest::new("model", json!({"unexpected":"object"}))
                .payload()
                .is_err()
        );
    }

    #[test]
    fn parser_preserves_quota_codes_without_retaining_raw_debug_body() {
        let event = parse_sse_frame(
            b"event: response.failed\ndata: {\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_limit_exceeded\",\"param\":\"input\"}}}\n",
        )
        .unwrap()
        .unwrap();

        let failure = event.failure().unwrap();
        assert_eq!(
            failure.code.as_deref(),
            Some("subscription_sharing_usage_limit_exceeded")
        );
        assert_eq!(failure.param.as_deref(), Some("input"));
        assert!(!format!("{event:?}").contains("subscription_sharing_usage_limit_exceeded"));
    }

    #[test]
    fn parser_handles_multiline_data_and_all_sse_line_endings() {
        let event =
            parse_sse_frame(b"event: response.completed\r\ndata: {\r\ndata: \"id\":\"r1\"}\r\n")
                .unwrap()
                .unwrap();

        assert!(event.is_completed());
        assert_eq!(event.data["id"], "r1");
    }

    #[tokio::test]
    async fn stream_requires_and_yields_a_completed_response() {
        let response = mock_response(
            "200 OK",
            "event: response.output_text.delta\ndata: {\"delta\":\"answer\"}\n\nevent: response.completed\ndata: {\"response\":{\"id\":\"resp_1\"}}\n\n",
        )
        .await;
        let mut stream = ResponsesStream::from_response(response).unwrap();

        assert_eq!(
            stream.next().await.unwrap().unwrap().event_type,
            "response.output_text.delta"
        );
        assert!(stream.next().await.unwrap().unwrap().is_completed());
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn failed_event_retains_machine_quota_code_without_body_logging() {
        let response = mock_response(
            "200 OK",
            "event: response.failed\ndata: {\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_limit_exceeded\",\"param\":\"input\"}}}\n\n",
        )
        .await;
        let mut stream = ResponsesStream::from_response(response).unwrap();
        let error = stream.next().await.unwrap().unwrap_err();

        let ProviderError::ResponsesRejected(failure) = error else {
            panic!("expected structured Responses failure");
        };
        assert_eq!(
            crate::classify_responses_failure(&failure),
            crate::ChatGptPlanStatus::UsageLimitReached
        );
    }

    #[tokio::test]
    async fn stream_without_terminal_success_is_rejected() {
        let response =
            mock_response("200 OK", "event: response.output_text.delta\ndata: {}\n\n").await;
        let mut stream = ResponsesStream::from_response(response).unwrap();

        assert!(stream.next().await.unwrap().is_ok());
        assert!(matches!(
            stream.next().await.unwrap(),
            Err(ProviderError::IncompleteResponse)
        ));
    }
}
