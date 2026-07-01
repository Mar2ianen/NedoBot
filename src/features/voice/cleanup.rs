use serde::Deserialize;

use crate::config::Config;
use crate::features::voice::types::{
    AsrTranscript, CleanTranscript, TranscriptChapter, TranscriptRenderMode,
};
use crate::llm::service::generate_text_with_provider;

const CLEANUP_PROMPT: &str = include_str!("../../../prompts/voice_cleanup.md");

pub async fn cleanup_transcript(
    config: &Config,
    transcript: &AsrTranscript,
) -> anyhow::Result<CleanTranscript> {
    let prompt = build_prompt(config, transcript);
    let generated = generate_text_with_provider(
        config,
        config.voice_cleanup_provider.as_deref(),
        config.voice_cleanup_model.as_deref(),
        &prompt,
        None,
        config.voice_cleanup_temperature,
        config.voice_cleanup_max_tokens,
    )
    .await?;

    parse_cleanup_json(&generated.content).or_else(|err| {
        tracing::warn!(%err, "failed to parse voice cleanup JSON, using plain LLM text");
        Ok(plain_cleanup(
            &generated.content,
            config.voice_short_text_max_chars,
        ))
    })
}

fn build_prompt(config: &Config, transcript: &AsrTranscript) -> String {
    let segments = if transcript.segments.is_empty() {
        transcript.text.clone()
    } else {
        transcript
            .segments
            .iter()
            .map(|segment| {
                format!(
                    "[{}-{}] {}",
                    format_timestamp(segment.start_sec),
                    format_timestamp(segment.end_sec),
                    segment.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "{CLEANUP_PROMPT}\n\nSHORT_LIMIT={}\n\nRAW_TEXT:\n{}\n\nSEGMENTS:\n{}",
        config.voice_short_text_max_chars, transcript.text, segments
    )
}

fn parse_cleanup_json(value: &str) -> anyhow::Result<CleanTranscript> {
    let json = strip_code_fence(value.trim());
    let response: CleanupResponse = serde_json::from_str(json)?;
    let text = response.text.trim().to_string();
    if text.is_empty() {
        anyhow::bail!("cleanup response text is empty");
    }

    let mode = match response.mode.as_deref() {
        Some("chapters") => TranscriptRenderMode::Chapters,
        Some("file") => TranscriptRenderMode::File,
        _ => TranscriptRenderMode::Short,
    };

    Ok(CleanTranscript {
        mode,
        text,
        chapters: response
            .chapters
            .into_iter()
            .map(|chapter| TranscriptChapter {
                title: chapter.title.trim().to_string(),
                start_sec: parse_timestamp(&chapter.start).unwrap_or(0.0),
                end_sec: chapter.end.as_deref().and_then(parse_timestamp),
                text: chapter.text.trim().to_string(),
            })
            .filter(|chapter| !chapter.title.is_empty() && !chapter.text.is_empty())
            .collect(),
        short_summary: response.short_summary.map(|value| value.trim().to_string()),
    })
}

fn plain_cleanup(value: &str, short_limit: usize) -> CleanTranscript {
    let text = value.trim().to_string();
    CleanTranscript {
        mode: if text.chars().count() <= short_limit {
            TranscriptRenderMode::Short
        } else {
            TranscriptRenderMode::Chapters
        },
        text,
        chapters: Vec::new(),
        short_summary: None,
    }
}

fn strip_code_fence(value: &str) -> &str {
    let value = value.trim();
    let Some(rest) = value.strip_prefix("```") else {
        return value;
    };
    let rest = rest
        .strip_prefix("json")
        .or_else(|| rest.strip_prefix("JSON"))
        .unwrap_or(rest)
        .trim_start();
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

fn parse_timestamp(value: &str) -> Option<f32> {
    let parts = value.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        [minutes, seconds] => {
            let minutes = minutes.parse::<f32>().ok()?;
            let seconds = seconds.parse::<f32>().ok()?;
            Some(minutes * 60.0 + seconds)
        }
        [hours, minutes, seconds] => {
            let hours = hours.parse::<f32>().ok()?;
            let minutes = minutes.parse::<f32>().ok()?;
            let seconds = seconds.parse::<f32>().ok()?;
            Some(hours * 3600.0 + minutes * 60.0 + seconds)
        }
        [seconds] => seconds.parse::<f32>().ok(),
        _ => None,
    }
}

fn format_timestamp(seconds: f32) -> String {
    let seconds = seconds.max(0.0).round() as u32;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[derive(Debug, Deserialize)]
struct CleanupResponse {
    mode: Option<String>,
    text: String,
    #[serde(default)]
    chapters: Vec<CleanupChapter>,
    short_summary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CleanupChapter {
    title: String,
    start: String,
    end: Option<String>,
    text: String,
}
