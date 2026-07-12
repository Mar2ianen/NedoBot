use serde::Deserialize;
use std::collections::BTreeSet;

use crate::config::Config;
use crate::features::voice::types::{
    AsrTranscript, CleanTranscript, TranscriptChapter, TranscriptRenderMode,
};
use crate::llm::service::generate_text_with_provider_and_system;

const CLEANUP_PROMPT: &str = include_str!("../../../prompts/voice_cleanup.md");
const MIN_CLEANUP_CONTENT_PERCENT: usize = 65;
const MAX_CLEANUP_CONTENT_PERCENT: usize = 135;

pub async fn cleanup_transcript(
    config: &Config,
    transcript: &AsrTranscript,
) -> anyhow::Result<CleanTranscript> {
    let prompt = build_user_prompt(config.voice_short_text_max_chars, transcript);
    let content = match generate_cleanup_content(config, &prompt).await {
        Ok(content) => content,
        Err(err) => {
            tracing::warn!(%err, "all voice cleanup providers failed, using raw ASR transcript");
            return Ok(normalize_cleanup(
                plain_cleanup(&transcript.text),
                transcript,
                config.voice_short_text_max_chars,
            ));
        }
    };

    let clean = match parse_cleanup_json(&content)
        .and_then(|clean| validate_cleanup_against_asr(&clean, transcript).map(|_| clean))
    {
        Ok(clean) => clean,
        Err(err) => {
            tracing::warn!(%err, "voice cleanup changed ASR beyond safe limits, using raw ASR transcript");
            plain_cleanup(&transcript.text)
        }
    };

    Ok(normalize_cleanup(
        clean,
        transcript,
        config.voice_short_text_max_chars,
    ))
}

async fn generate_cleanup_content(config: &Config, prompt: &str) -> anyhow::Result<String> {
    generate_cleanup_with_provider(
        config,
        config.voice_cleanup_provider.as_deref(),
        config.voice_cleanup_model.as_deref(),
        prompt,
    )
    .await
}

async fn generate_cleanup_with_provider(
    config: &Config,
    provider: Option<&str>,
    model: Option<&str>,
    prompt: &str,
) -> anyhow::Result<String> {
    Ok(generate_text_with_provider_and_system(
        config,
        provider,
        model,
        Some(CLEANUP_PROMPT),
        prompt,
        None,
        config.voice_cleanup_temperature,
        config.voice_cleanup_max_tokens,
    )
    .await?
    .content)
}

fn build_user_prompt(short_limit: usize, transcript: &AsrTranscript) -> String {
    let source = if transcript.segments.is_empty() {
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

    format!("SHORT_LIMIT={short_limit}\n\nSOURCE_TRANSCRIPT:\n{source}")
}

fn validate_cleanup_against_asr(
    clean: &CleanTranscript,
    transcript: &AsrTranscript,
) -> anyhow::Result<()> {
    validate_cleanup_text(&transcript.text, &clean.text)?;

    if !clean.chapters.is_empty() {
        let chapters = clean
            .chapters
            .iter()
            .map(|chapter| chapter.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        validate_cleanup_text(&transcript.text, &chapters)?;
    }

    Ok(())
}

fn validate_cleanup_text(raw: &str, cleaned: &str) -> anyhow::Result<()> {
    let raw_len = content_len(raw);
    let cleaned_len = content_len(cleaned);
    if raw_len >= 100
        && (cleaned_len * 100 < raw_len * MIN_CLEANUP_CONTENT_PERCENT
            || cleaned_len * 100 > raw_len * MAX_CLEANUP_CONTENT_PERCENT)
    {
        anyhow::bail!(
            "cleanup content length changed too much: raw={raw_len}, cleaned={cleaned_len}"
        );
    }

    let raw_numbers = number_tokens(raw);
    let added_numbers = number_tokens(cleaned)
        .difference(&raw_numbers)
        .cloned()
        .collect::<Vec<_>>();
    if !added_numbers.is_empty() {
        anyhow::bail!("cleanup introduced numbers not present in ASR: {added_numbers:?}");
    }

    Ok(())
}

fn content_len(text: &str) -> usize {
    text.chars().filter(|ch| ch.is_alphanumeric()).count()
}

fn number_tokens(text: &str) -> BTreeSet<String> {
    text.split(|ch: char| !ch.is_ascii_digit())
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
        .collect()
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

fn plain_cleanup(value: &str) -> CleanTranscript {
    let text = value.trim().to_string();
    CleanTranscript {
        mode: TranscriptRenderMode::Short,
        text,
        chapters: Vec::new(),
        short_summary: None,
    }
}

fn normalize_cleanup(
    mut clean: CleanTranscript,
    _transcript: &AsrTranscript,
    short_limit: usize,
) -> CleanTranscript {
    clean.text = normalize_terms(&clean.text);
    for chapter in &mut clean.chapters {
        chapter.title = normalize_terms(&chapter.title);
        chapter.text = normalize_terms(&chapter.text);
    }

    if clean.text.chars().count() <= short_limit || clean.chapters.is_empty() {
        clean.mode = TranscriptRenderMode::Short;
        clean.chapters.clear();
        return clean;
    }

    if clean.mode == TranscriptRenderMode::Short {
        clean.mode = TranscriptRenderMode::Chapters;
    }

    clean
}

fn normalize_terms(text: &str) -> String {
    let replacements = [
        ("Грок", "groq"),
        ("грок", "groq"),
        ("Groq", "groq"),
        ("Clean up", "cleanup"),
        ("clean up", "cleanup"),
        ("Clean Up", "cleanup"),
        ("клин ап", "cleanup"),
        ("клинап", "cleanup"),
        ("LLM", "LLM"),
        ("Оллама", "ollama"),
        ("оллама", "ollama"),
        ("Гемма", "Gemma"),
        ("гемма", "Gemma"),
        ("Церебрас", "Cerebras"),
        ("церебрас", "Cerebras"),
        ("салфразиты", "слова-паразиты"),
    ];

    replacements
        .into_iter()
        .fold(text.trim().to_string(), |acc, (from, to)| {
            acc.replace(from, to)
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_cleanup_without_model_chapters_stays_plain_text() {
        let transcript = AsrTranscript {
            provider: "groq".to_string(),
            model: "whisper".to_string(),
            request_id: None,
            text: "Длинный текст без явного деления на темы.".to_string(),
            segments: Vec::new(),
            raw_json: serde_json::json!({}),
        };
        let clean = CleanTranscript {
            mode: TranscriptRenderMode::Short,
            text: "Длинный текст без явного деления на темы.".to_string(),
            chapters: Vec::new(),
            short_summary: None,
        };

        let normalized = normalize_cleanup(clean, &transcript, 20);
        assert_eq!(normalized.mode, TranscriptRenderMode::Short);
        assert!(normalized.chapters.is_empty());
    }

    #[test]
    fn normalize_terms_keeps_provider_names_latin() {
        let transcript = AsrTranscript {
            provider: "groq".to_string(),
            model: "whisper".to_string(),
            request_id: None,
            text: "грок и клинап".to_string(),
            segments: Vec::new(),
            raw_json: serde_json::json!({}),
        };
        let clean = CleanTranscript {
            mode: TranscriptRenderMode::Short,
            text: "Грок и клинап через гемма.".to_string(),
            chapters: Vec::new(),
            short_summary: None,
        };

        let normalized = normalize_cleanup(clean, &transcript, 400);
        assert_eq!(normalized.text, "groq и cleanup через Gemma.");
    }

    #[test]
    fn prompt_uses_timestamped_segments_without_duplicate_raw_text() {
        let transcript = AsrTranscript {
            provider: "groq".to_string(),
            model: "whisper".to_string(),
            request_id: None,
            text: "дублировать этот текст не надо".to_string(),
            segments: vec![crate::features::voice::types::AsrSegment {
                start_sec: 2.0,
                end_sec: 5.0,
                text: "единственный источник".to_string(),
            }],
            raw_json: serde_json::json!({}),
        };

        let prompt = build_user_prompt(400, &transcript);

        assert!(prompt.contains("SOURCE_TRANSCRIPT"));
        assert!(prompt.contains("[0:02-0:05] единственный источник"));
        assert!(!prompt.contains(&transcript.text));
    }

    #[test]
    fn cleanup_rejects_new_numbers_and_excessive_rewrite() {
        let raw = "Gemma 4 31B ".repeat(40);
        let transcript = transcript_with_text(&raw);
        let with_new_number = plain_cleanup(&format!("{} 2027", raw));
        let rewritten = plain_cleanup("короткое резюме");

        assert!(validate_cleanup_against_asr(&with_new_number, &transcript).is_err());
        assert!(validate_cleanup_against_asr(&rewritten, &transcript).is_err());
    }

    #[test]
    fn cleanup_accepts_punctuation_edits_without_new_numbers() {
        let transcript = transcript_with_text("Вышла Gemma 4 31B и новый драйвер для Vulkan.");
        let clean = plain_cleanup("Вышла Gemma 4 31B, и новый драйвер для Vulkan.");

        assert!(validate_cleanup_against_asr(&clean, &transcript).is_ok());
    }

    fn transcript_with_text(text: &str) -> AsrTranscript {
        AsrTranscript {
            provider: "groq".to_string(),
            model: "whisper".to_string(),
            request_id: None,
            text: text.to_string(),
            segments: Vec::new(),
            raw_json: serde_json::json!({}),
        }
    }
}
