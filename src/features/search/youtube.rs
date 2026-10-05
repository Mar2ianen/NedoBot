use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;

use crate::config::Config;
use crate::features::search::types::{MAX_RESULT_SNIPPET_CHARS, SearchResult};

const MAX_SUBTITLE_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct YoutubeSubtitlesConfig {
    pub(crate) enabled: bool,
    pub(crate) command: Option<String>,
    pub(crate) languages: Vec<String>,
    pub(crate) timeout_sec: u64,
    pub(crate) max_chars: usize,
    pub(crate) max_videos: usize,
}

#[allow(dead_code)]
impl YoutubeSubtitlesConfig {
    pub(crate) fn disabled() -> Self {
        Self {
            enabled: false,
            command: Some("yt-dlp".to_string()),
            languages: vec![
                "ru".to_string(),
                "ru.*".to_string(),
                "en".to_string(),
                "en.*".to_string(),
            ],
            timeout_sec: 15,
            max_chars: 12_000,
            max_videos: 2,
        }
    }

    pub(crate) fn from_bot_config(config: &Config) -> Self {
        Self {
            enabled: config.youtube_subtitles_enabled,
            command: config.youtube_subtitles_command.clone(),
            languages: config.youtube_subtitles_languages.clone(),
            timeout_sec: config.youtube_subtitles_timeout_sec,
            max_chars: config.youtube_subtitles_max_chars,
            max_videos: config.youtube_subtitles_max_videos,
        }
    }

    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let command = self
            .command
            .as_deref()
            .filter(|command| !command.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("YouTube subtitles command is not configured"))?;
        if !command_is_available(command) {
            anyhow::bail!("YouTube subtitles command was not found on PATH");
        }
        if self.languages.is_empty()
            || self
                .languages
                .iter()
                .any(|language| language.trim().is_empty())
        {
            anyhow::bail!("YouTube subtitles languages must contain a non-empty selector");
        }
        if self.timeout_sec == 0 {
            anyhow::bail!("YouTube subtitles timeout must be greater than 0");
        }
        if self.max_chars == 0 {
            anyhow::bail!("YouTube subtitles max chars must be greater than 0");
        }
        if self.max_videos == 0 {
            anyhow::bail!("YouTube subtitles max videos must be greater than 0");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct YoutubeTranscript {
    pub(crate) language: String,
    pub(crate) text: String,
}

/// Adds a bounded transcript to search results returned by an external MCP.
///
/// yt-dlp is invoked with argument passing (not through a shell) and only for
/// recognized public YouTube URLs. Media downloads are disabled; the temporary
/// directory contains subtitle files only and is removed when this function
/// returns.
pub async fn enrich_results_with_youtube_subtitles(config: &Config, results: &mut [SearchResult]) {
    let config = YoutubeSubtitlesConfig::from_bot_config(config);
    enrich_results_with_config(&config, results).await;
}

pub(crate) async fn enrich_results_with_config(
    config: &YoutubeSubtitlesConfig,
    results: &mut [SearchResult],
) {
    if !config.enabled {
        return;
    }

    let mut processed_videos = 0;
    for result in results.iter_mut() {
        if processed_videos >= config.max_videos {
            break;
        }
        if !is_youtube_video_url(&result.url) {
            continue;
        }
        processed_videos += 1;

        match get_youtube_subtitles(config, &result.url).await {
            Ok(Some(transcript)) => prepend_transcript(result, transcript, config.max_chars),
            Ok(None) => tracing::debug!(url = %result.url, "YouTube subtitles unavailable"),
            Err(error) => tracing::warn!(
                %error,
                url = %result.url,
                "YouTube subtitle extraction failed"
            ),
        }
    }
}

pub(crate) fn is_youtube_video_url(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value.trim()) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }

    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "youtu.be" {
        return first_path_segment(&url).is_some();
    }
    if !matches_youtube_host(&host) {
        return false;
    }

    let path_segments = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    match path_segments.as_slice() {
        ["watch", ..] => url
            .query_pairs()
            .any(|(key, value)| key == "v" && !value.trim().is_empty()),
        [kind, video_id, ..] if matches!(*kind, "shorts" | "live" | "embed" | "v" | "clip") => {
            !video_id.trim().is_empty()
        }
        _ => false,
    }
}

fn matches_youtube_host(host: &str) -> bool {
    host == "youtube.com"
        || host.ends_with(".youtube.com")
        || host == "youtube-nocookie.com"
        || host.ends_with(".youtube-nocookie.com")
}

fn first_path_segment(url: &reqwest::Url) -> Option<&str> {
    url.path_segments()?
        .find(|segment| !segment.is_empty())
        .filter(|segment| !segment.trim().is_empty())
}

async fn fetch_youtube_subtitles(
    config: &YoutubeSubtitlesConfig,
    url: &str,
) -> anyhow::Result<Option<YoutubeTranscript>> {
    let command_name = config
        .command
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("YouTube subtitles command is not configured"))?;
    let language_selectors = config.languages.join(",");
    let temporary_directory = tempfile::tempdir()?;
    let output_template = temporary_directory
        .path()
        .join("%(id)s.%(ext)s")
        .to_string_lossy()
        .into_owned();

    let mut child = Command::new(command_name)
        .args([
            "--skip-download",
            "--no-playlist",
            "--no-warnings",
            "--quiet",
            "--write-subs",
            "--write-auto-subs",
            "--sub-format",
            "vtt",
            "--sub-langs",
            &language_selectors,
            "--output",
            &output_template,
            url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    match timeout(Duration::from_secs(config.timeout_sec), child.wait()).await {
        Ok(status) => {
            let _ = status?;
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Ok(None);
        }
    }

    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(temporary_directory.path())? {
        let path = entry?.path();
        if !is_vtt_file(&path) {
            continue;
        }
        let metadata = std::fs::metadata(&path)?;
        if metadata.len() > MAX_SUBTITLE_FILE_BYTES {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let text = parse_vtt(&contents, config.max_chars);
        if text.is_empty() {
            continue;
        }
        candidates.push((subtitle_language(&path), path, text));
    }

    candidates.sort_by(|left, right| {
        language_rank(&left.0, &config.languages)
            .cmp(&language_rank(&right.0, &config.languages))
            .then_with(|| left.1.cmp(&right.1))
    });

    Ok(candidates
        .into_iter()
        .next()
        .map(|(language, _, text)| YoutubeTranscript { language, text }))
}

pub(crate) async fn get_youtube_subtitles(
    config: &YoutubeSubtitlesConfig,
    url: &str,
) -> anyhow::Result<Option<YoutubeTranscript>> {
    fetch_youtube_subtitles(config, url).await
}

#[allow(dead_code)]
fn command_is_available(command: &str) -> bool {
    let path = Path::new(command);
    if command.contains(std::path::MAIN_SEPARATOR) {
        return path.is_file();
    }

    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .any(|directory| directory.join(command).is_file())
}

fn is_vtt_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("vtt"))
}

fn subtitle_language(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.rsplit_once('.').map(|(_, language)| language))
        .filter(|language| !language.trim().is_empty())
        .unwrap_or("unknown")
        .to_string()
}

fn language_rank(language: &str, selectors: &[String]) -> usize {
    let language = language.to_ascii_lowercase();
    selectors
        .iter()
        .position(|selector| language_matches_selector(&language, selector))
        .unwrap_or(selectors.len())
}

fn language_matches_selector(language: &str, selector: &str) -> bool {
    let selector = selector.trim().to_ascii_lowercase();
    if selector == "all" || selector == language {
        return true;
    }
    let Some(prefix) = selector.strip_suffix(".*") else {
        return false;
    };
    language == prefix
        || language.starts_with(&format!("{prefix}-"))
        || language.starts_with(&format!("{prefix}_"))
}

pub(crate) fn parse_vtt(value: &str, max_chars: usize) -> String {
    let mut text = String::new();
    let mut skip_block = false;
    let mut previous_line = String::new();

    for raw_line in value.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            skip_block = false;
            continue;
        }
        if line.starts_with("WEBVTT") {
            continue;
        }
        if line == "NOTE" || line.starts_with("NOTE ") || line == "STYLE" || line == "REGION" {
            skip_block = true;
            continue;
        }
        if skip_block || line.contains("-->") || line.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }

        let line = normalize_caption_line(line);
        if line.is_empty() || line == previous_line {
            continue;
        }
        previous_line = line.clone();

        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&line);
        if text.chars().count() >= max_chars {
            break;
        }
    }

    text.chars().take(max_chars).collect()
}

fn normalize_caption_line(value: &str) -> String {
    let mut result = String::new();
    let mut in_angle_tag = false;
    let mut in_voice_position = false;
    for character in value.chars() {
        match character {
            '<' => in_angle_tag = true,
            '>' if in_angle_tag => in_angle_tag = false,
            '{' => in_voice_position = true,
            '}' if in_voice_position => in_voice_position = false,
            _ if in_angle_tag || in_voice_position => {}
            _ => result.push(character),
        }
    }

    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn prepend_transcript(result: &mut SearchResult, transcript: YoutubeTranscript, max_chars: usize) {
    let max_chars = max_chars.min(MAX_RESULT_SNIPPET_CHARS);
    let transcript_text = transcript.text.chars().take(max_chars).collect::<String>();
    let language = if transcript.language == "unknown" {
        "auto".to_string()
    } else {
        transcript.language
    };
    let combined = if result.snippet.trim().is_empty() {
        format!("YouTube subtitles ({language}): {transcript_text}")
    } else {
        format!(
            "YouTube subtitles ({language}): {transcript_text} Search result: {}",
            result.snippet.trim()
        )
    };
    result.snippet = combined.chars().take(MAX_RESULT_SNIPPET_CHARS).collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_supported_youtube_video_urls() {
        for url in [
            "https://www.youtube.com/watch?v=abc123",
            "https://youtu.be/abc123?t=42",
            "https://www.youtube.com/shorts/abc123",
            "https://www.youtube-nocookie.com/embed/abc123",
        ] {
            assert!(is_youtube_video_url(url), "expected YouTube URL: {url}");
        }
    }

    #[test]
    fn rejects_non_video_and_unsafe_youtube_urls() {
        for url in [
            "https://www.youtube.com/",
            "https://www.youtube.com/playlist?list=abc123",
            "https://example.com/watch?v=abc123",
            "https://user:pass@www.youtube.com/watch?v=abc123",
            "file:///tmp/video",
        ] {
            assert!(!is_youtube_video_url(url), "must reject URL: {url}");
        }
    }

    #[test]
    fn parses_vtt_without_timing_markup_or_duplicate_lines() {
        let vtt = "WEBVTT\n\n00:00.000 --> 00:01.000\n<c>Привет &amp; мир</c>\n\n00:01.000 --> 00:02.000\n<c>Привет &amp; мир</c>\n\n00:02.000 --> 00:03.000\n{\\an8}Продолжение\n";

        assert_eq!(parse_vtt(vtt, 100), "Привет & мир Продолжение");
    }

    #[test]
    fn prefers_requested_language_order() {
        let selectors = vec!["ru".to_string(), "ru.*".to_string(), "en".to_string()];

        assert_eq!(language_rank("en", &selectors), 2);
        assert_eq!(language_rank("ru-orig", &selectors), 1);
        assert!(language_matches_selector("ru-ua", "ru.*"));
        assert!(!language_matches_selector("rus", "ru.*"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn enriches_youtube_result_with_yt_dlp_vtt() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temporary directory must be created");
        let command = directory.path().join("fake-yt-dlp");
        std::fs::write(
            &command,
            r##"#!/bin/sh
output=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then
    shift
    output="$1"
  fi
  shift
done
output=$(printf '%s' "$output" | sed 's/%(id)s/fake-id/; s/\.%(ext)s//')
printf '%s\n' 'WEBVTT' '' '00:00.000 --> 00:01.000' '<c>Привет &amp; мир</c>' > "$output.ru.vtt"
"##,
        )
        .expect("fake yt-dlp must be written");
        let mut permissions = std::fs::metadata(&command)
            .expect("fake yt-dlp metadata must be readable")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&command, permissions).expect("fake yt-dlp must be executable");

        let mut config = Config::from_env().expect("test configuration must parse");
        config.youtube_subtitles_enabled = true;
        config.youtube_subtitles_command = Some(command.to_string_lossy().into_owned());
        config.youtube_subtitles_languages = vec!["ru".to_string(), "en".to_string()];
        config.youtube_subtitles_timeout_sec = 2;
        config.youtube_subtitles_max_chars = 100;
        config.youtube_subtitles_max_videos = 1;

        let mut results = vec![SearchResult {
            source: crate::features::search::types::SearchSource::Web,
            title: "Video".to_string(),
            url: "https://www.youtube.com/watch?v=abc123".to_string(),
            snippet: "Найдено во внешнем MCP".to_string(),
        }];
        enrich_results_with_youtube_subtitles(&config, &mut results).await;

        assert_eq!(
            results[0].snippet,
            "YouTube subtitles (ru): Привет & мир Search result: Найдено во внешнем MCP"
        );
    }
}
