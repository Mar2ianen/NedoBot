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

    let bytes = tokio::fs::read(path).await?;
    let mut file_part = Part::bytes(bytes).file_name(filename.to_string());
    if let Some(mime_type) = mime_type {
        file_part = file_part.mime_str(mime_type)?;
    }

    let form = Form::new()
        .text("model", config.voice_asr_model.clone())
        .text("response_format", "verbose_json")
        .text("language", config.voice_language.clone())
        .text("temperature", config.voice_asr_temperature.to_string())
        .text("timestamp_granularities[]", "segment")
        .part("file", file_part);

    let response = http::client(Duration::from_secs(120))?
        .post(GROQ_TRANSCRIPTIONS_URL)
        .bearer_auth(config.groq_api_key.trim())
        .multipart(form)
        .send()
        .await?
        .error_for_status()?
        .json::<GroqTranscriptionResponse>()
        .await?;

    let raw_json = serde_json::to_value(&response)?;
    let text = response.text.trim().to_string();

    Ok(AsrTranscript {
        provider,
        model: config.voice_asr_model.clone(),
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
