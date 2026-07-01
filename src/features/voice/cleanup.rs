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

    let clean = parse_cleanup_json(&generated.content).or_else(|err| {
        tracing::warn!(%err, "failed to parse voice cleanup JSON, using plain LLM text");
        Ok::<CleanTranscript, anyhow::Error>(plain_cleanup(
            &generated.content,
            config.voice_short_text_max_chars,
        ))
    })?;

    Ok(normalize_cleanup(
        clean,
        transcript,
        config.voice_short_text_max_chars,
    ))
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

fn normalize_cleanup(
    mut clean: CleanTranscript,
    transcript: &AsrTranscript,
    short_limit: usize,
) -> CleanTranscript {
    clean.text = normalize_terms(&clean.text);
    for chapter in &mut clean.chapters {
        chapter.title = normalize_terms(&chapter.title);
        chapter.text = normalize_terms(&chapter.text);
    }

    if clean.text.chars().count() <= short_limit {
        clean.mode = TranscriptRenderMode::Short;
        clean.chapters.clear();
        return clean;
    }

    if clean.mode == TranscriptRenderMode::Short || clean.chapters.is_empty() {
        clean.mode = TranscriptRenderMode::Chapters;
        clean.chapters = fallback_chapters(transcript, &clean.text);
    }

    clean
}

fn fallback_chapters(transcript: &AsrTranscript, clean_text: &str) -> Vec<TranscriptChapter> {
    let chunks = split_clean_chunks(clean_text, 260);
    if chunks.is_empty() {
        return vec![TranscriptChapter {
            title: fallback_title(clean_text),
            start_sec: 0.0,
            end_sec: None,
            text: clean_text.trim().to_string(),
        }];
    }

    let total_start = transcript
        .segments
        .first()
        .map(|segment| segment.start_sec)
        .unwrap_or(0.0);
    let total_end = transcript
        .segments
        .last()
        .map(|segment| segment.end_sec)
        .unwrap_or(total_start);
    let span = (total_end - total_start).max(0.0);
    let count = chunks.len().max(1) as f32;

    chunks
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            let start_sec = total_start + span * index as f32 / count;
            let end_sec = Some(total_start + span * (index + 1) as f32 / count);
            TranscriptChapter {
                title: fallback_title(&text),
                start_sec,
                end_sec,
                text,
            }
        })
        .collect()
}

fn split_clean_chunks(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for sentence in split_sentences(text) {
        let needs_space = !current.is_empty();
        let projected_len =
            current.chars().count() + sentence.chars().count() + usize::from(needs_space);
        if projected_len > max_chars && !current.is_empty() {
            chunks.push(current.trim().to_string());
            current.clear();
        }

        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(sentence);
    }

    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }

    chunks
}

fn split_sentences(text: &str) -> Vec<&str> {
    let mut sentences = Vec::new();
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        if matches!(ch, '.' | '!' | '?') {
            let end = index + ch.len_utf8();
            let sentence = text[start..end].trim();
            if !sentence.is_empty() {
                sentences.push(sentence);
            }
            start = end;
        }
    }

    let rest = text[start..].trim();
    if !rest.is_empty() {
        sentences.push(rest);
    }
    sentences
}

fn fallback_title(text: &str) -> String {
    if let Some(title) = keyword_title(text) {
        return title.to_string();
    }

    let stop_words = [
        "так",
        "вообще",
        "короче",
        "вот",
        "ну",
        "типа",
        "значит",
        "по",
        "идее",
        "мне",
        "надо",
        "что-то",
        "чего-то",
        "наговорить",
        "приколу",
        "сейчас",
        "это",
        "вот",
        "этих",
    ];
    let words = text
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|ch: char| {
                ch.is_ascii_punctuation() || matches!(ch, '«' | '»' | '“' | '”')
            })
        })
        .filter(|word| !word.is_empty())
        .filter(|word| {
            let lower = word.to_lowercase();
            !stop_words.contains(&lower.as_str())
        })
        .take(5)
        .collect::<Vec<_>>();

    if words.is_empty() {
        "Фрагмент".to_string()
    } else {
        words.join(" ")
    }
}

fn keyword_title(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    let contains_any = |needles: &[&str]| needles.iter().any(|needle| lower.contains(needle));

    if contains_any(&["размет", "размеч", "размещ", "глав", "блок"]) {
        return Some("Разметка текста");
    }
    if contains_any(&["cleanup", "clean up", "клинап", "чистить", "очищ"]) {
        return Some("Cleanup текста");
    }
    if contains_any(&["слова-параз", "паразит"]) {
        return Some("Слова-паразиты");
    }
    if contains_any(&["памят"]) {
        return Some("Память и нагрузка");
    }
    if contains_any(&["groq", "грок", "whisper", "asr", "транскрип"]) {
        return Some("ASR и расшифровка");
    }
    if contains_any(&["нейрон", "модель", "llm"]) {
        return Some("LLM-обработка");
    }
    if contains_any(&["провер"]) {
        return Some("Проверка расшифровки");
    }

    None
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
    use crate::features::voice::types::AsrSegment;

    #[test]
    fn long_short_cleanup_is_forced_into_chapters() {
        let transcript = AsrTranscript {
            provider: "groq".to_string(),
            model: "whisper".to_string(),
            request_id: None,
            text: "Первый фрагмент про проверку. Второй фрагмент про разметку.".to_string(),
            segments: vec![
                AsrSegment {
                    start_sec: 0.0,
                    end_sec: 3.0,
                    text: "Первый фрагмент про проверку.".to_string(),
                },
                AsrSegment {
                    start_sec: 3.0,
                    end_sec: 6.0,
                    text: "Второй фрагмент про разметку.".to_string(),
                },
            ],
            raw_json: serde_json::json!({}),
        };
        let clean = CleanTranscript {
            mode: TranscriptRenderMode::Short,
            text: "Первый фрагмент про проверку. Второй фрагмент про разметку.".to_string(),
            chapters: Vec::new(),
            short_summary: None,
        };

        let normalized = normalize_cleanup(clean, &transcript, 20);
        assert_eq!(normalized.mode, TranscriptRenderMode::Chapters);
        assert!(!normalized.chapters.is_empty());
    }

    #[test]
    fn fallback_chapters_use_clean_text_not_raw_segments() {
        let transcript = AsrTranscript {
            provider: "groq".to_string(),
            model: "whisper".to_string(),
            request_id: None,
            text: "так короче сырой текст".to_string(),
            segments: vec![AsrSegment {
                start_sec: 0.0,
                end_sec: 24.0,
                text: "так короче сырой текст".to_string(),
            }],
            raw_json: serde_json::json!({}),
        };
        let chapters = fallback_chapters(
            &transcript,
            "Система должна нормально размечать текст. Cleanup должен убрать слова-паразиты.",
        );

        assert_eq!(chapters[0].title, "Разметка текста");
        assert!(chapters[0].text.contains("Cleanup"));
        assert!(!chapters[0].text.contains("сырой текст"));
    }
}
