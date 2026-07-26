use std::path::Path;
use std::time::Duration;

use reqwest::multipart::{Form, Part};
use serde::Deserialize;

use crate::config::Config;
use crate::features::voice::types::{AsrSegment, AsrTranscript};
use crate::http;

const GROQ_TRANSCRIPTIONS_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";

pub async fn transcribe_audio(
    config: &Config,
    path: &Path,
    filename: &str,
    mime_type: Option<&str>,
) -> anyhow::Result<AsrTranscript> {
    let provider = config.voice_asr_provider.trim().to_lowercase();
    if provider != "groq" {
        anyhow::bail!("unsupported VOICE_ASR_PROVIDER: {provider}");
    }
    if config.groq_api_key.trim().is_empty() {
        anyhow::bail!("GROQ_API_KEY is empty");
    }

    transcribe_groq_audio(
        path,
        filename,
        mime_type,
        config.groq_api_key.trim(),
        &config.voice_asr_model,
        &config.voice_language,
        config.voice_asr_temperature,
        GROQ_TRANSCRIPTIONS_URL,
    )
    .await
}

async fn transcribe_groq_audio(
    path: &Path,
    filename: &str,
    mime_type: Option<&str>,
    api_key: &str,
    model: &str,
    language: &str,
    temperature: f32,
    endpoint: &str,
) -> anyhow::Result<AsrTranscript> {
    let bytes = tokio::fs::read(path).await?;
    let mut file_part = Part::bytes(bytes).file_name(filename.to_string());
    if let Some(mime_type) = mime_type {
        file_part = file_part.mime_str(mime_type)?;
    }

    let form = Form::new()
        .text("model", model.to_string())
        .text("response_format", "verbose_json")
        .text("language", language.to_string())
        .text("temperature", temperature.to_string())
        .text("timestamp_granularities[]", "segment")
        .part("file", file_part);

    let response = http::client(Duration::from_secs(120))?
        .post(endpoint)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await?
        .error_for_status()?
        .json::<GroqTranscriptionResponse>()
        .await?;

    let raw_json = serde_json::to_value(&response)?;
    let text = response.text.trim().to_string();

    Ok(AsrTranscript {
        provider: "groq".to_string(),
        model: model.to_string(),
        request_id: response.x_groq.and_then(|value| value.id),
        text,
        segments: response
            .segments
            .into_iter()
            .map(|segment| AsrSegment {
                start_sec: segment.start,
                end_sec: segment.end,
                text: segment.text.trim().to_string(),
            })
            .filter(|segment| !segment.text.is_empty())
            .collect(),
        raw_json,
    })
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct GroqTranscriptionResponse {
    text: String,
    #[serde(default)]
    segments: Vec<GroqSegment>,
    #[serde(default)]
    x_groq: Option<GroqRequestMeta>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct GroqSegment {
    #[serde(default)]
    start: f32,
    #[serde(default)]
    end: f32,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct GroqRequestMeta {
    id: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{Json, Router, body::Bytes, extract::State, http::HeaderMap, routing::post};
    use serde_json::json;

    use super::*;

    #[derive(Debug)]
    struct CapturedRequest {
        authorization: Option<String>,
        content_type: Option<String>,
        body: Bytes,
    }

    async fn mock_groq_transcription(
        State(captured): State<Arc<Mutex<Option<CapturedRequest>>>>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Json<serde_json::Value> {
        *captured.lock().unwrap() = Some(CapturedRequest {
            authorization: headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            content_type: headers
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            body,
        });

        Json(json!({
            "text": " Привет, мир ",
            "segments": [{
                "start": 0.0,
                "end": 1.25,
                "text": " Привет, мир "
            }],
            "x_groq": { "id": "req_mock_123" }
        }))
    }

    #[tokio::test]
    async fn groq_asr_multipart_request_contains_required_wire_fields() {
        let captured = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route("/audio/transcriptions", post(mock_groq_transcription))
            .with_state(Arc::clone(&captured));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let audio_bytes = b"mock-opus-audio\0bytes";
        let audio_file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(audio_file.path(), audio_bytes).unwrap();
        let endpoint = format!("http://{address}/audio/transcriptions");

        let transcript = transcribe_groq_audio(
            audio_file.path(),
            "voice-message.ogg",
            Some("audio/ogg"),
            "test-groq-key",
            "whisper-large-v3-turbo",
            "ru",
            0.0,
            &endpoint,
        )
        .await
        .unwrap();

        let request = captured.lock().unwrap().take().unwrap();
        server.abort();

        assert_eq!(
            request.authorization.as_deref(),
            Some("Bearer test-groq-key")
        );
        assert!(
            request
                .content_type
                .as_deref()
                .is_some_and(|value| value.starts_with("multipart/form-data; boundary="))
        );

        let body = String::from_utf8_lossy(&request.body);
        assert!(body.contains("name=\"model\"\r\n\r\nwhisper-large-v3-turbo"));
        assert!(body.contains("name=\"response_format\"\r\n\r\nverbose_json"));
        assert!(body.contains("name=\"language\"\r\n\r\nru"));
        assert!(body.contains("name=\"timestamp_granularities[]\"\r\n\r\nsegment"));
        assert!(body.contains("name=\"file\"; filename=\"voice-message.ogg\""));
        assert!(
            request
                .body
                .windows(audio_bytes.len())
                .any(|bytes| bytes == audio_bytes)
        );

        assert_eq!(transcript.provider, "groq");
        assert_eq!(transcript.model, "whisper-large-v3-turbo");
        assert_eq!(transcript.request_id.as_deref(), Some("req_mock_123"));
        assert_eq!(transcript.text, "Привет, мир");
        assert_eq!(
            transcript.segments,
            vec![AsrSegment {
                start_sec: 0.0,
                end_sec: 1.25,
                text: "Привет, мир".to_string(),
            }]
        );
        assert_eq!(transcript.raw_json["x_groq"]["id"], "req_mock_123");
    }
}
