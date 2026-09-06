use std::path::Path;
use std::time::Duration;

use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use serde_json::Value;

use crate::config::Config;
use crate::features::voice::types::{AsrSegment, AsrTranscript};
use crate::http;

const GROQ_TRANSCRIPTIONS_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
const GEMINI_FILES_UPLOAD_URL: &str =
    "https://generativelanguage.googleapis.com/upload/v1beta/files";
const GEMINI_INTERACTIONS_URL: &str =
    "https://generativelanguage.googleapis.com/v1beta/interactions";
const GEMINI_TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(180);

pub async fn transcribe_audio_with_shadow(
    config: &Config,
    path: &Path,
    filename: &str,
    mime_type: Option<&str>,
) -> anyhow::Result<(AsrTranscript, Vec<AsrTranscript>)> {
    let primary_provider = config.voice_asr_provider.trim().to_ascii_lowercase();
    let shadow_provider = shadow_provider_for(&primary_provider);
    let shadow_mime_type = shadow_audio_mime_type(filename, mime_type);
    let shadow_enabled = config.voice_asr_shadow_enabled && shadow_provider.is_some();
    let primary = transcribe_audio(config, path, filename, mime_type);
    let shadow = async {
        if !shadow_enabled {
            return Ok(None);
        }
        let shadow_provider = shadow_provider.expect("shadow provider checked above");
        if shadow_provider == "gemini" && shadow_mime_type.is_none() {
            return Ok(None);
        }
        transcribe_configured_audio(
            config,
            path,
            filename,
            mime_type,
            shadow_provider,
            &config.voice_asr_shadow_model,
        )
        .await
        .map(Some)
    };
    let (primary, shadow) = tokio::join!(primary, shadow);
    match primary {
        Ok(primary) => {
            let alternatives = match shadow {
                Ok(Some(transcript)) => vec![transcript],
                Ok(None) => Vec::new(),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        model = %config.voice_asr_shadow_model,
                        "shadow ASR failed; keeping primary transcript"
                    );
                    Vec::new()
                }
            };
            Ok((primary, alternatives))
        }
        Err(primary_error) => match shadow {
            Ok(Some(fallback)) => {
                tracing::warn!(
                    error = %primary_error,
                    primary = %primary_provider,
                    fallback = %fallback.provider,
                    "primary ASR failed; using the second transcript"
                );
                Ok((fallback, Vec::new()))
            }
            Ok(None) => Err(primary_error),
            Err(shadow_error) => {
                tracing::warn!(
                    %shadow_error,
                    primary = %primary_provider,
                    "primary and shadow ASR both failed"
                );
                Err(primary_error)
            }
        },
    }
}

pub async fn transcribe_audio(
    config: &Config,
    path: &Path,
    filename: &str,
    mime_type: Option<&str>,
) -> anyhow::Result<AsrTranscript> {
    transcribe_configured_audio(
        config,
        path,
        filename,
        mime_type,
        config.voice_asr_provider.trim(),
        &config.voice_asr_model,
    )
    .await
}

async fn transcribe_configured_audio(
    config: &Config,
    path: &Path,
    filename: &str,
    mime_type: Option<&str>,
    provider: &str,
    model: &str,
) -> anyhow::Result<AsrTranscript> {
    let provider = provider.trim().to_ascii_lowercase();
    let model = model.trim();
    if model.is_empty() {
        anyhow::bail!("ASR model is empty for provider {provider}");
    }

    match provider.as_str() {
        "groq" => {
            if config.groq_api_key.trim().is_empty() {
                anyhow::bail!("GROQ_API_KEY is empty");
            }
            transcribe_groq_audio(GroqTranscriptionRequest {
                path,
                filename,
                mime_type,
                api_key: config.groq_api_key.trim(),
                model,
                language: &config.voice_language,
                temperature: config.voice_asr_temperature,
                endpoint: GROQ_TRANSCRIPTIONS_URL,
            })
            .await
        }
        "gemini" => {
            let mime_type = shadow_audio_mime_type(filename, mime_type)
                .ok_or_else(|| anyhow::anyhow!("Gemini ASR requires an audio MIME type"))?;
            transcribe_gemini_audio(config, path, &mime_type, model).await
        }
        provider => anyhow::bail!("unsupported VOICE_ASR_PROVIDER: {provider}"),
    }
}

fn shadow_provider_for(primary_provider: &str) -> Option<&'static str> {
    match primary_provider {
        "groq" => Some("gemini"),
        "gemini" => Some("groq"),
        _ => None,
    }
}

struct GroqTranscriptionRequest<'a> {
    path: &'a Path,
    filename: &'a str,
    mime_type: Option<&'a str>,
    api_key: &'a str,
    model: &'a str,
    language: &'a str,
    temperature: f32,
    endpoint: &'a str,
}

async fn transcribe_groq_audio(
    request: GroqTranscriptionRequest<'_>,
) -> anyhow::Result<AsrTranscript> {
    let bytes = tokio::fs::read(request.path).await?;
    let mut file_part = Part::bytes(bytes).file_name(request.filename.to_string());
    if let Some(mime_type) = request.mime_type {
        file_part = file_part.mime_str(mime_type)?;
    }

    let form = Form::new()
        .text("model", request.model.to_string())
        .text("response_format", "verbose_json")
        .text("language", request.language.to_string())
        .text("temperature", request.temperature.to_string())
        .text("timestamp_granularities[]", "segment")
        .part("file", file_part);

    let response = http::client(Duration::from_secs(120))?
        .post(request.endpoint)
        .bearer_auth(request.api_key)
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
        model: request.model.to_string(),
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

async fn transcribe_gemini_audio(
    config: &Config,
    path: &Path,
    mime_type: &str,
    model: &str,
) -> anyhow::Result<AsrTranscript> {
    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|_| anyhow::anyhow!("GEMINI_API_KEY is not configured for shadow ASR"))?;
    let api_key = api_key.trim();
    if api_key.is_empty() {
        anyhow::bail!("GEMINI_API_KEY is empty for shadow ASR");
    }

    let bytes = tokio::fs::read(path).await?;
    let client =
        crate::http::client_with_proxy(GEMINI_TRANSCRIBE_TIMEOUT, config.llm_proxy_url.as_deref())?;
    let upload_url = start_gemini_upload(&client, api_key, bytes.len(), mime_type).await?;
    let file = finalize_gemini_upload(&client, &upload_url, bytes, mime_type).await?;
    let interaction = match create_gemini_transcription(
        &client,
        api_key,
        &file,
        model,
        &config.voice_language,
        mime_type,
    )
    .await
    {
        Ok(interaction) => interaction,
        Err(error) => {
            delete_gemini_file(&client, api_key, &file.name).await;
            return Err(error);
        }
    };
    delete_gemini_file(&client, api_key, &file.name).await;

    let text = extract_gemini_transcription_text(&interaction)
        .ok_or_else(|| anyhow::anyhow!("Gemini Transcribe returned no text output"))?;
    let request_id = interaction
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Ok(AsrTranscript {
        provider: "gemini".to_string(),
        model: model.to_string(),
        request_id,
        text,
        segments: Vec::new(),
        raw_json: interaction,
    })
}

async fn start_gemini_upload(
    client: &reqwest::Client,
    api_key: &str,
    content_length: usize,
    mime_type: &str,
) -> anyhow::Result<String> {
    let response = client
        .post(GEMINI_FILES_UPLOAD_URL)
        .header("x-goog-api-key", api_key)
        .header("X-Goog-Upload-Protocol", "resumable")
        .header("X-Goog-Upload-Command", "start")
        .header(
            "X-Goog-Upload-Header-Content-Length",
            content_length.to_string(),
        )
        .header("X-Goog-Upload-Header-Content-Type", mime_type)
        .json(&serde_json::json!({
            "file": {"display_name": "nedobot-shadow-asr"}
        }))
        .send()
        .await?
        .error_for_status()?;
    response
        .headers()
        .get("x-goog-upload-url")
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow::anyhow!("Gemini Files API returned no upload URL"))
}

async fn finalize_gemini_upload(
    client: &reqwest::Client,
    upload_url: &str,
    bytes: Vec<u8>,
    mime_type: &str,
) -> anyhow::Result<GeminiFile> {
    let content_length = bytes.len();
    let response = client
        .post(upload_url)
        .header("Content-Length", content_length.to_string())
        .header("X-Goog-Upload-Offset", "0")
        .header("X-Goog-Upload-Command", "upload, finalize")
        .header("Content-Type", mime_type)
        .body(bytes)
        .send()
        .await?
        .error_for_status()?
        .json::<GeminiFileUploadResponse>()
        .await?;
    Ok(response.file)
}

async fn create_gemini_transcription(
    client: &reqwest::Client,
    api_key: &str,
    file: &GeminiFile,
    model: &str,
    language: &str,
    mime_type: &str,
) -> anyhow::Result<Value> {
    let language_codes = gemini_language_codes(language);
    let response = client
        .post(GEMINI_INTERACTIONS_URL)
        .header("x-goog-api-key", api_key)
        .json(&serde_json::json!({
            "model": model,
            "input": [{
                "type": "audio",
                "uri": file.uri,
                "mime_type": mime_type
            }],
            "generation_config": {
                "transcription_config": {
                    "language_codes": language_codes,
                    "mode": {"type": "verbatim"}
                }
            }
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    Ok(response)
}

async fn delete_gemini_file(client: &reqwest::Client, api_key: &str, name: &str) {
    if let Err(error) = client
        .delete(format!("{GEMINI_FILES_UPLOAD_URL_BASE}/{name}"))
        .header("x-goog-api-key", api_key)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
    {
        tracing::debug!(%error, file = %name, "failed to delete shadow ASR upload");
    }
}

const GEMINI_FILES_UPLOAD_URL_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

fn extract_gemini_transcription_text(value: &Value) -> Option<String> {
    if let Some(text) = value.get("output_text").and_then(Value::as_str) {
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }

    fn collect_text(value: &Value, parts: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                if object.get("type").and_then(Value::as_str) == Some("text")
                    && let Some(text) = object.get("text").and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    parts.push(text.trim().to_string());
                }
                for nested in object.values() {
                    collect_text(nested, parts);
                }
            }
            Value::Array(items) => {
                for item in items {
                    collect_text(item, parts);
                }
            }
            _ => {}
        }
    }

    let mut parts = Vec::new();
    collect_text(value.get("steps")?, &mut parts);
    if parts.is_empty() {
        collect_text(value.get("outputs")?, &mut parts);
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn gemini_language_codes(language: &str) -> Vec<String> {
    let language = language.trim();
    if language.is_empty() {
        return Vec::new();
    }
    let code = match language.to_ascii_lowercase().as_str() {
        "ru" => "ru-RU",
        "en" => "en-US",
        "uk" => "uk-UA",
        "be" => "be-BY",
        _ => language,
    };
    vec![code.to_string()]
}

fn shadow_audio_mime_type(filename: &str, mime_type: Option<&str>) -> Option<String> {
    if mime_type.is_some_and(|value| value.starts_with("audio/")) {
        return mime_type.map(ToOwned::to_owned);
    }
    match filename
        .rsplit_once('.')
        .map(|(_, suffix)| suffix.to_ascii_lowercase())
    {
        Some(suffix) if suffix == "ogg" || suffix == "oga" => Some("audio/ogg".to_string()),
        Some(suffix) if suffix == "opus" => Some("audio/opus".to_string()),
        Some(suffix) if suffix == "mp3" => Some("audio/mp3".to_string()),
        Some(suffix) if suffix == "m4a" => Some("audio/m4a".to_string()),
        Some(suffix) if suffix == "wav" => Some("audio/wav".to_string()),
        Some(suffix) if suffix == "flac" => Some("audio/flac".to_string()),
        Some(suffix) if suffix == "webm" => Some("audio/webm".to_string()),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct GeminiFileUploadResponse {
    file: GeminiFile,
}

#[derive(Debug, Deserialize)]
struct GeminiFile {
    name: String,
    uri: String,
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

        let transcript = transcribe_groq_audio(GroqTranscriptionRequest {
            path: audio_file.path(),
            filename: "voice-message.ogg",
            mime_type: Some("audio/ogg"),
            api_key: "test-groq-key",
            model: "whisper-large-v3-turbo",
            language: "ru",
            temperature: 0.0,
            endpoint: &endpoint,
        })
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

    #[test]
    fn extracts_text_from_interactions_steps() {
        let response = serde_json::json!({
            "status": "completed",
            "steps": [{
                "content": [{"type": "text", "text": "  Первая фраза. "}]
            }, {
                "content": [{"type": "text", "text": "Вторая фраза."}]
            }]
        });

        assert_eq!(
            extract_gemini_transcription_text(&response).as_deref(),
            Some("Первая фраза.\nВторая фраза.")
        );
    }

    #[test]
    fn shadow_audio_mime_type_rejects_video_and_infers_audio_suffix() {
        assert_eq!(
            shadow_audio_mime_type("telegram-voice.oga", None).as_deref(),
            Some("audio/ogg")
        );
        assert_eq!(
            shadow_audio_mime_type("telegram-video.mp4", Some("video/mp4")),
            None
        );
        assert_eq!(
            shadow_audio_mime_type("telegram-audio.bin", Some("audio/mpeg")).as_deref(),
            Some("audio/mpeg")
        );
    }

    #[test]
    fn gemini_language_codes_map_project_language_to_bcp47() {
        assert_eq!(gemini_language_codes("ru"), vec!["ru-RU"]);
        assert_eq!(gemini_language_codes(""), Vec::<String>::new());
        assert_eq!(gemini_language_codes("en-US"), vec!["en-US"]);
    }

    #[test]
    fn dual_mode_uses_the_other_asr_provider_as_shadow() {
        assert_eq!(shadow_provider_for("gemini"), Some("groq"));
        assert_eq!(shadow_provider_for("groq"), Some("gemini"));
        assert_eq!(shadow_provider_for("unknown"), None);
    }
}
