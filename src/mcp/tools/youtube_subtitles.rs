//! Bounded YouTube subtitle retrieval through the configured yt-dlp executable.

use std::{env, path::Path, time::Duration};

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use tokio::{process::Command, time::timeout};
use url::Url;

use super::{invalid_arguments, read_error};

pub const ENABLED_ENV: &str = "MCP_YOUTUBE_SUBTITLES_ENABLED";
pub const COMMAND_ENV: &str = "MCP_YOUTUBE_SUBTITLES_COMMAND";
pub const LANGUAGES_ENV: &str = "MCP_YOUTUBE_SUBTITLES_LANGUAGES";
pub const TIMEOUT_ENV: &str = "MCP_YOUTUBE_SUBTITLES_TIMEOUT_SEC";
pub const MAX_CHARS_ENV: &str = "MCP_YOUTUBE_SUBTITLES_MAX_CHARS";
pub const MAX_VIDEOS_ENV: &str = "MCP_YOUTUBE_SUBTITLES_MAX_VIDEOS";

const DEFAULT_LANGUAGES: &str = "ru,ru.*,en,en.*";
const MAX_TIMEOUT_SECONDS: u64 = 120;
const MAX_TEXT_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct YoutubeSubtitlesConfig {
    pub command: String,
    pub languages: String,
    pub timeout: Duration,
    pub max_chars: usize,
    pub max_videos: usize,
}

impl YoutubeSubtitlesConfig {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let enabled = env::var(ENABLED_ENV).unwrap_or_else(|_| "false".to_owned());
        let enabled = match enabled.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => true,
            "false" | "0" | "no" => false,
            _ => anyhow::bail!("{ENABLED_ENV} must be a boolean"),
        };
        if !enabled {
            return Ok(None);
        }

        let command = env::var(COMMAND_ENV)
            .map_err(|_| anyhow::anyhow!("{COMMAND_ENV} is required when subtitles are enabled"))?;
        anyhow::ensure!(
            !command.trim().is_empty(),
            "{COMMAND_ENV} must not be empty"
        );
        anyhow::ensure!(
            Path::new(&command).is_absolute(),
            "{COMMAND_ENV} must be an absolute executable path"
        );
        let languages = env::var(LANGUAGES_ENV).unwrap_or_else(|_| DEFAULT_LANGUAGES.to_owned());
        validate_languages(&languages)?;
        let timeout_seconds = parse_bounded_env(TIMEOUT_ENV, 15, 1, MAX_TIMEOUT_SECONDS)?;
        let max_chars = parse_bounded_env(MAX_CHARS_ENV, 12_000, 256, 50_000)? as usize;
        let max_videos = parse_bounded_env(MAX_VIDEOS_ENV, 2, 1, 5)? as usize;

        Ok(Some(Self {
            command,
            languages,
            timeout: Duration::from_secs(timeout_seconds),
            max_chars,
            max_videos,
        }))
    }
}

fn parse_bounded_env(name: &str, default: u64, min: u64, max: u64) -> anyhow::Result<u64> {
    let value = env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("{name} must be an integer"))
        })
        .transpose()?
        .unwrap_or(default);
    anyhow::ensure!(
        (min..=max).contains(&value),
        "{name} must be in {min}..={max}"
    );
    Ok(value)
}

fn validate_languages(value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !value.trim().is_empty(),
        "{LANGUAGES_ENV} must not be empty"
    );
    anyhow::ensure!(
        value.split(',').all(|language| {
            !language.is_empty()
                && language
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'*' | b'-'))
        }),
        "{LANGUAGES_ENV} contains an invalid language selector"
    );
    Ok(())
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetSubtitlesInput {
    /// HTTPS URLs from youtube.com or youtu.be; playlists and non-video paths are rejected.
    pub urls: Vec<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct YoutubeSubtitle {
    pub video_id: String,
    pub url: String,
    pub transcript: Option<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct GetSubtitlesOutput {
    pub videos: Vec<YoutubeSubtitle>,
}

pub async fn get_subtitles(
    config: Option<&YoutubeSubtitlesConfig>,
    input: GetSubtitlesInput,
) -> Result<GetSubtitlesOutput, rmcp::ErrorData> {
    let Some(config) = config else {
        return Err(read_error("YouTube subtitles are disabled"));
    };
    if input.urls.is_empty() || input.urls.len() > config.max_videos {
        return Err(invalid_arguments(format!(
            "urls must contain between 1 and {} video URLs",
            config.max_videos
        )));
    }

    let mut videos = Vec::with_capacity(input.urls.len());
    let mut remaining_chars = config.max_chars;
    for requested_url in input.urls {
        let Some((video_id, canonical_url)) = canonical_video_url(&requested_url) else {
            return Err(invalid_arguments(
                "only direct HTTPS YouTube video URLs are accepted",
            ));
        };
        let transcript = fetch_one(config, &canonical_url).await?;
        let (transcript, note) = match transcript {
            Some(text) if remaining_chars > 0 => {
                let text = truncate_chars(&text, remaining_chars);
                remaining_chars = remaining_chars.saturating_sub(text.chars().count());
                (Some(text), None)
            }
            Some(_) => (None, Some("общий лимит текста достигнут".to_owned())),
            None => (
                None,
                Some("субтитры на выбранных языках не найдены".to_owned()),
            ),
        };
        videos.push(YoutubeSubtitle {
            video_id,
            url: canonical_url,
            transcript,
            note,
        });
    }
    Ok(GetSubtitlesOutput { videos })
}

pub fn canonical_video_url(value: &str) -> Option<(String, String)> {
    let url = Url::parse(value.trim()).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let path_segments = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let id = match host.as_str() {
        "youtu.be" if path_segments.len() == 1 => path_segments[0].to_string(),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" => {
            match path_segments.as_slice() {
                ["watch"] => url
                    .query_pairs()
                    .find(|(key, _)| key == "v")?
                    .1
                    .into_owned(),
                [kind, id] if matches!(*kind, "shorts" | "embed" | "live") => (*id).to_owned(),
                _ => return None,
            }
        }
        _ => return None,
    };
    if id.len() != 11
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some((id.clone(), format!("https://www.youtube.com/watch?v={id}")))
}

async fn fetch_one(
    config: &YoutubeSubtitlesConfig,
    canonical_url: &str,
) -> Result<Option<String>, rmcp::ErrorData> {
    let temp_dir =
        tempfile::tempdir().map_err(|_| read_error("subtitle temp directory unavailable"))?;
    let output_template = temp_dir.path().join("%(id)s.%(language)s.%(ext)s");
    let output_future = Command::new(&config.command)
        .args([
            "--ignore-config",
            "--no-cache-dir",
            "--no-warnings",
            "--quiet",
            "--no-progress",
            "--no-playlist",
            "--skip-download",
            "--write-subs",
            "--write-auto-subs",
            "--sub-langs",
            &config.languages,
            "--sub-format",
            "vtt",
            "--max-downloads",
            "1",
            "--output",
        ])
        .arg(output_template)
        .arg("--")
        .arg(canonical_url)
        .kill_on_drop(true)
        .output();
    let output = timeout(config.timeout, output_future)
        .await
        .map_err(|_| read_error("subtitle provider timed out"))?
        .map_err(|_| read_error("subtitle provider could not be started"))?;
    if !output.status.success() {
        return Err(read_error("subtitle provider failed"));
    }

    let mut files = tokio::fs::read_dir(temp_dir.path())
        .await
        .map_err(|_| read_error("subtitle output unavailable"))?;
    let mut candidates = Vec::new();
    while let Some(entry) = files
        .next_entry()
        .await
        .map_err(|_| read_error("subtitle output unavailable"))?
    {
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "vtt")
        {
            candidates.push(entry.path());
        }
    }
    candidates.sort_by_key(|path| language_priority(path, &config.languages));
    for path in candidates {
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|_| read_error("subtitle output unavailable"))?;
        if metadata.len() > MAX_TEXT_FILE_BYTES {
            continue;
        }
        let contents = tokio::fs::read_to_string(path)
            .await
            .map_err(|_| read_error("subtitle output was not valid UTF-8"))?;
        let text = parse_vtt(&contents);
        if !text.is_empty() {
            return Ok(Some(text));
        }
    }
    Ok(None)
}

fn language_priority(path: &Path, configured: &str) -> usize {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    configured
        .split(',')
        .position(|language| {
            let prefix = language.trim_end_matches(".*");
            filename
                .split('.')
                .any(|part| part == prefix || part.starts_with(&format!("{prefix}-")))
        })
        .unwrap_or(usize::MAX)
}

pub fn parse_vtt(input: &str) -> String {
    let mut lines = Vec::new();
    let mut in_cue = false;
    for raw_line in input.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            in_cue = false;
            continue;
        }
        if line.contains("-->") {
            in_cue = true;
            continue;
        }
        if !in_cue || line == "WEBVTT" || line.starts_with("NOTE") {
            continue;
        }
        let clean = strip_vtt_tags(line);
        if !clean.is_empty() && lines.last().is_none_or(|previous| previous != &clean) {
            lines.push(clean);
        }
    }
    lines.join("\n")
}

fn strip_vtt_tags(value: &str) -> String {
    let mut clean = String::with_capacity(value.len());
    let mut inside_tag = false;
    for character in value.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => clean.push(character),
            _ => {}
        }
    }
    clean.trim().to_owned()
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_direct_youtube_video_urls() {
        assert_eq!(
            canonical_video_url("https://youtu.be/abcdefghijk?t=30"),
            Some((
                "abcdefghijk".to_owned(),
                "https://www.youtube.com/watch?v=abcdefghijk".to_owned()
            ))
        );
        assert_eq!(
            canonical_video_url("https://www.youtube.com/shorts/abcdefghijk"),
            Some((
                "abcdefghijk".to_owned(),
                "https://www.youtube.com/watch?v=abcdefghijk".to_owned()
            ))
        );
        for url in [
            "http://youtu.be/abcdefghijk",
            "https://youtube.com/playlist?list=abcdefghijk",
            "https://youtube.com.evil.test/watch?v=abcdefghijk",
            "https://user@youtube.com/watch?v=abcdefghijk",
            "https://youtube.com/watch?v=too-short",
        ] {
            assert_eq!(canonical_video_url(url), None, "accepted {url}");
        }
    }

    #[test]
    fn parses_vtt_cues_strips_markup_and_deduplicates_adjacent_text() {
        let vtt = "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\n<c>Привет</c>\n\n00:00:01.000 --> 00:00:02.000\nПривет\n\n00:00:02.000 --> 00:00:03.000\nМир";
        assert_eq!(parse_vtt(vtt), "Привет\nМир");
    }

    #[test]
    fn rejects_injected_language_selectors() {
        assert!(validate_languages("ru,ru.*,en").is_ok());
        assert!(validate_languages("ru --exec=touch").is_err());
    }

    #[tokio::test]
    async fn rejects_invalid_hosts_before_starting_the_provider() {
        let config = YoutubeSubtitlesConfig {
            command: "/definitely/missing/yt-dlp".to_owned(),
            languages: DEFAULT_LANGUAGES.to_owned(),
            timeout: Duration::from_secs(1),
            max_chars: 1000,
            max_videos: 2,
        };
        let error = get_subtitles(
            Some(&config),
            GetSubtitlesInput {
                urls: vec!["https://example.com/watch?v=abcdefghijk".to_owned()],
            },
        )
        .await
        .expect_err("non-YouTube URLs must be rejected before subprocess launch");
        assert_eq!(
            error.message,
            "only direct HTTPS YouTube video URLs are accepted"
        );
    }
}
