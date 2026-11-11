//! Bounded YouTube subtitle retrieval through the configured yt-dlp executable.

use std::{collections::VecDeque, env, path::Path, process::Stdio, sync::Arc, time::Duration};

use crate::config::{
    YOUTUBE_SUBTITLES_MAX_PAGE_CHARS, YOUTUBE_SUBTITLES_MAX_TIMEOUT_SEC,
    YOUTUBE_SUBTITLES_MAX_VIDEOS, YOUTUBE_SUBTITLES_MIN_PAGE_CHARS,
    youtube_language_selectors_are_valid,
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    sync::{Mutex, Semaphore},
    time::{Instant, timeout},
};
use url::Url;

use super::{invalid_arguments, read_error};

pub const ENABLED_ENV: &str = "MCP_YOUTUBE_SUBTITLES_ENABLED";
pub const COMMAND_ENV: &str = "MCP_YOUTUBE_SUBTITLES_COMMAND";
pub const LANGUAGES_ENV: &str = "MCP_YOUTUBE_SUBTITLES_LANGUAGES";
pub const TIMEOUT_ENV: &str = "MCP_YOUTUBE_SUBTITLES_TIMEOUT_SEC";
pub const MAX_CHARS_ENV: &str = "MCP_YOUTUBE_SUBTITLES_MAX_CHARS";
pub const MAX_VIDEOS_ENV: &str = "MCP_YOUTUBE_SUBTITLES_MAX_VIDEOS";

const DEFAULT_LANGUAGES: &str = "ru,en";
const MAX_TEXT_FILE_BYTES: u64 = 8 * 1024 * 1024;
const CACHE_CAPACITY: usize = 8;
const CACHE_TTL: Duration = Duration::from_secs(3600);
const DEFAULT_PAGE_CHARS: usize = 6000;

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
        let timeout_seconds =
            parse_bounded_env(TIMEOUT_ENV, 60, 1, YOUTUBE_SUBTITLES_MAX_TIMEOUT_SEC)?;
        let max_chars = parse_bounded_env(
            MAX_CHARS_ENV,
            12_000,
            YOUTUBE_SUBTITLES_MIN_PAGE_CHARS as u64,
            YOUTUBE_SUBTITLES_MAX_PAGE_CHARS as u64,
        )? as usize;
        let max_videos =
            parse_bounded_env(MAX_VIDEOS_ENV, 2, 1, YOUTUBE_SUBTITLES_MAX_VIDEOS as u64)? as usize;

        Ok(Some(Self {
            command,
            languages,
            timeout: Duration::from_secs(timeout_seconds),
            max_chars,
            max_videos,
        }))
    }
}

/// Полные публичные субтитры живут только в памяти этого MCP-инстанса.
pub struct YoutubeSubtitlesService {
    config: YoutubeSubtitlesConfig,
    cache: Mutex<VecDeque<CachedTranscript>>,
    fetch_permit: Semaphore,
}

struct CachedTranscript {
    video_id: String,
    loaded_at: Instant,
    transcript: Arc<Transcript>,
}

struct Transcript {
    text: String,
    language: String,
    id: String,
    subtitle_end_seconds: Option<f64>,
}

impl YoutubeSubtitlesService {
    pub fn new(config: YoutubeSubtitlesConfig) -> Self {
        Self {
            config,
            cache: Mutex::new(VecDeque::new()),
            fetch_permit: Semaphore::new(1),
        }
    }

    async fn cached(&self, video_id: &str) -> Option<Arc<Transcript>> {
        let mut cache = self.cache.lock().await;
        cache.retain(|item| item.loaded_at.elapsed() < CACHE_TTL);
        let index = cache.iter().position(|item| item.video_id == video_id)?;
        let item = cache.remove(index)?;
        let transcript = Arc::clone(&item.transcript);
        cache.push_back(item);
        Some(transcript)
    }

    async fn load(
        &self,
        video_id: &str,
        url: &str,
    ) -> Result<Option<Arc<Transcript>>, rmcp::ErrorData> {
        if let Some(transcript) = self.cached(video_id).await {
            return Ok(Some(transcript));
        }
        // Не создаём очередь subprocess-ов/ожидающих задач при массовых запросах.
        let _permit = self
            .fetch_permit
            .try_acquire()
            .map_err(|_| read_error("subtitle provider is busy; retry shortly"))?;
        if let Some(transcript) = self.cached(video_id).await {
            return Ok(Some(transcript));
        }
        let Some(transcript) = fetch_one(&self.config, url).await? else {
            return Ok(None);
        };
        let transcript = Arc::new(transcript);
        let mut cache = self.cache.lock().await;
        if cache.len() >= CACHE_CAPACITY {
            cache.pop_front();
        }
        cache.push_back(CachedTranscript {
            video_id: video_id.to_owned(),
            loaded_at: Instant::now(),
            transcript: Arc::clone(&transcript),
        });
        Ok(Some(transcript))
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

pub(crate) fn validate_languages(value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !value.trim().is_empty(),
        "{LANGUAGES_ENV} must not be empty"
    );
    anyhow::ensure!(
        youtube_language_selectors_are_valid(value),
        "{LANGUAGES_ENV} contains an invalid language selector"
    );
    Ok(())
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetSubtitlesInput {
    /// HTTPS URLs from youtube.com or youtu.be; playlists and non-video paths are rejected.
    pub urls: Vec<String>,
    /// Смещение в символах Unicode; для продолжения передайте next_offset выбранного видео.
    #[serde(default)]
    pub offset: usize,
    /// Общий бюджет страницы, не всего ролика. По умолчанию до 6000 символов.
    pub max_chars: Option<usize>,
    /// Идентификатор полного текста из предыдущей страницы; исключает смешивание разных версий.
    pub transcript_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct YoutubeSubtitle {
    pub video_id: String,
    pub url: String,
    pub transcript: Option<String>,
    pub note: Option<String>,
    pub language: Option<String>,
    pub transcript_id: Option<String>,
    pub total_chars: usize,
    pub offset: usize,
    pub returned_chars: usize,
    pub has_more: bool,
    pub next_offset: Option<usize>,
    pub subtitle_end_seconds: Option<f64>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct GetSubtitlesOutput {
    pub videos: Vec<YoutubeSubtitle>,
}

pub async fn get_subtitles(
    service: Option<&YoutubeSubtitlesService>,
    input: GetSubtitlesInput,
) -> Result<GetSubtitlesOutput, rmcp::ErrorData> {
    let Some(service) = service else {
        return Err(read_error("YouTube subtitles are disabled"));
    };
    let config = &service.config;
    if input.urls.is_empty() || input.urls.len() > config.max_videos {
        return Err(invalid_arguments(format!(
            "urls must contain between 1 and {} video URLs",
            config.max_videos
        )));
    }

    let urls = input
        .urls
        .iter()
        .map(|requested| {
            canonical_video_url(requested).ok_or_else(|| {
                invalid_arguments("only direct HTTPS YouTube video URLs are accepted")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let budget = input
        .max_chars
        .unwrap_or(DEFAULT_PAGE_CHARS.min(config.max_chars));
    if budget == 0 || budget > config.max_chars {
        return Err(invalid_arguments(
            "max_chars exceeds the configured page budget",
        ));
    }
    if input
        .transcript_id
        .as_ref()
        .is_some_and(|id| id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(invalid_arguments("invalid transcript_id"));
    }
    if budget < urls.len() {
        return Err(invalid_arguments(
            "page budget must allow at least one character per video",
        ));
    }
    let mut videos = Vec::with_capacity(urls.len());
    let page_chars = budget / urls.len();
    for (video_id, canonical_url) in urls {
        let transcript = service.load(&video_id, &canonical_url).await?;
        let mut video = subtitle_page(
            transcript.as_deref(),
            input.offset,
            page_chars,
            input.transcript_id.as_deref(),
        )?;
        video.video_id = video_id;
        video.url = canonical_url;
        videos.push(video);
    }
    Ok(GetSubtitlesOutput { videos })
}

fn subtitle_page(
    transcript: Option<&Transcript>,
    offset: usize,
    limit: usize,
    expected_id: Option<&str>,
) -> Result<YoutubeSubtitle, rmcp::ErrorData> {
    let mut video = YoutubeSubtitle {
        video_id: String::new(),
        url: String::new(),
        transcript: None,
        note: None,
        language: None,
        transcript_id: None,
        total_chars: 0,
        offset,
        returned_chars: 0,
        has_more: false,
        next_offset: None,
        subtitle_end_seconds: None,
    };
    let Some(transcript) = transcript else {
        if expected_id.is_some() {
            return Err(read_error(
                "subtitle version is no longer available; restart at offset 0",
            ));
        }
        video.note = Some("субтитры на выбранных языках не найдены".to_owned());
        return Ok(video);
    };
    if expected_id.is_some_and(|id| id != transcript.id) {
        return Err(read_error("subtitle version changed; restart at offset 0"));
    }
    video.total_chars = transcript.text.chars().count();
    if offset > video.total_chars {
        return Err(invalid_arguments("offset exceeds transcript length"));
    }
    let text: String = transcript.text.chars().skip(offset).take(limit).collect();
    video.returned_chars = text.chars().count();
    video.has_more = offset + video.returned_chars < video.total_chars;
    video.next_offset = video.has_more.then_some(offset + video.returned_chars);
    video.note = video.has_more.then(|| {
        "Это часть субтитров. Для продолжения запросите этот URL с next_offset и transcript_id."
            .to_owned()
    });
    video.language = Some(transcript.language.clone());
    video.transcript_id = Some(transcript.id.clone());
    video.subtitle_end_seconds = transcript.subtitle_end_seconds;
    video.transcript = Some(text);
    Ok(video)
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
) -> Result<Option<Transcript>, rmcp::ErrorData> {
    let deadline = Instant::now() + config.timeout;
    // yt-dlp скачивает ВСЕ запрошенные языки. Ошибка перевода en после успешного
    // ru не должна ломать результат: выбираем первую доступную исходную дорожку.
    for language in config.languages.split(',') {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| read_error("subtitle provider timed out"))?;
        let single = YoutubeSubtitlesConfig {
            languages: language.to_owned(),
            timeout: remaining,
            ..config.clone()
        };
        if let Some(transcript) = fetch_one_language(&single, canonical_url).await? {
            return Ok(Some(transcript));
        }
    }
    Ok(None)
}

async fn fetch_one_language(
    config: &YoutubeSubtitlesConfig,
    canonical_url: &str,
) -> Result<Option<Transcript>, rmcp::ErrorData> {
    let temp_dir =
        tempfile::tempdir().map_err(|_| read_error("subtitle temp directory unavailable"))?;
    let output_template = temp_dir.path().join("%(id)s.%(language)s.%(ext)s");
    let mut child = Command::new(&config.command)
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
            "--extractor-args",
            "youtube:skip=translated_subs",
            "--sub-langs",
            &config.languages,
            "--sub-format",
            "vtt",
            "--socket-timeout",
            "20",
            "--retries",
            "1",
            "--output",
        ])
        .arg(output_template)
        .arg("--max-filesize")
        .arg(MAX_TEXT_FILE_BYTES.to_string())
        .arg("--")
        .arg(canonical_url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| read_error("subtitle provider could not be started"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| read_error("subtitle provider output unavailable"))?;
    let output_future = async {
        let drain = async {
            let mut prefix = Vec::new();
            (&mut stderr)
                .take(16 * 1024)
                .read_to_end(&mut prefix)
                .await?;
            tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await?;
            Ok::<_, std::io::Error>(prefix)
        };
        tokio::try_join!(child.wait(), drain)
    };
    let (status, stderr) = timeout(config.timeout, output_future)
        .await
        .map_err(|_| read_error("subtitle provider timed out"))?
        .map_err(|_| read_error("subtitle provider could not be started"))?;
    if !status.success() {
        return Err(read_error(
            if String::from_utf8_lossy(&stderr).contains("HTTP Error 429") {
                "YouTube rate-limited subtitle requests; try again later"
            } else {
                "subtitle provider failed; partial files were not returned"
            },
        ));
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
    candidates.sort_by_key(|path| (language_priority(path, &config.languages), path.clone()));
    for path in candidates {
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|_| read_error("subtitle output unavailable"))?;
        if metadata.len() > MAX_TEXT_FILE_BYTES {
            return Err(read_error("subtitle file exceeds the safe size limit"));
        }
        let contents = tokio::fs::read_to_string(&path)
            .await
            .map_err(|_| read_error("subtitle output was not valid UTF-8"))?;
        let text = parse_vtt(&contents);
        if !text.is_empty() {
            let language = path
                .file_stem()
                .and_then(|value| value.to_str())
                .and_then(|value| value.rsplit_once('.').map(|(_, language)| language))
                .unwrap_or("unknown")
                .to_owned();
            let id = Sha256::digest(format!("{language}\0{text}"))
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let subtitle_end_seconds = contents
                .lines()
                .filter_map(|line| line.split_once("-->"))
                .filter_map(|(_, end)| parse_timestamp(end.split_whitespace().next()?))
                .max_by(f64::total_cmp);
            return Ok(Some(Transcript {
                text,
                language,
                id,
                subtitle_end_seconds,
            }));
        }
    }
    Ok(None)
}

fn parse_timestamp(value: &str) -> Option<f64> {
    let mut seconds = 0.0;
    for part in value.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    (seconds.is_finite() && seconds >= 0.0).then_some(seconds)
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
    clean
        .trim()
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
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

    #[cfg(unix)]
    fn fixture_provider(vtt: &str, exit_code: i32) -> (tempfile::TempDir, YoutubeSubtitlesService) {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("source.vtt"), vtt).unwrap();
        let command = directory.path().join("yt-dlp-fixture");
        let script = format!(
            r##"#!/bin/sh
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then shift; output="$1"; fi
  shift
done
printf x >> "$root/calls"
cp "$root/source.vtt" "$(dirname -- "$output")/fixture.ru.vtt"
exit {exit_code}
"##
        );
        std::fs::write(&command, script).unwrap();
        std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o755)).unwrap();
        let service = YoutubeSubtitlesService::new(YoutubeSubtitlesConfig {
            command: command.to_str().unwrap().to_owned(),
            languages: "ru,en".to_owned(),
            timeout: Duration::from_secs(5),
            max_chars: 12_000,
            max_videos: 2,
        });
        (directory, service)
    }

    fn page_input(offset: usize, transcript_id: Option<String>) -> GetSubtitlesInput {
        GetSubtitlesInput {
            urls: vec!["https://youtu.be/abcdefghijk".to_owned()],
            offset,
            max_chars: Some(3000),
            transcript_id,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn complete_long_video_roundtrips_without_gaps_and_fetches_once() {
        let mut vtt = String::from("WEBVTT\n\n");
        for index in 0..240 {
            vtt.push_str(&format!(
                "00:00:00.000 --> 01:35:01.000\nФрагмент {index}: {} 🦀\n\n",
                "Кириллица &amp; продолжение. ".repeat(8)
            ));
        }
        let expected = parse_vtt(&vtt);
        assert!(expected.chars().count() > 50_000);
        let (directory, service) = fixture_provider(&vtt, 0);
        let mut offset = 0;
        let mut version = None;
        let mut recovered = String::new();
        loop {
            let output = get_subtitles(Some(&service), page_input(offset, version.clone()))
                .await
                .unwrap();
            let page = &output.videos[0];
            assert_eq!(page.offset, offset);
            assert_eq!(page.total_chars, expected.chars().count());
            assert_eq!(page.subtitle_end_seconds, Some(5701.0));
            assert_eq!(page.language.as_deref(), Some("ru"));
            let text = page.transcript.as_ref().unwrap();
            assert_eq!(page.returned_chars, text.chars().count());
            recovered.push_str(text);
            version = page.transcript_id.clone();
            if !page.has_more {
                assert!(page.next_offset.is_none());
                break;
            }
            assert!(page.note.is_some());
            let next = page.next_offset.unwrap();
            assert_eq!(next, offset + text.chars().count());
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(recovered, expected);
        assert_eq!(
            std::fs::read_to_string(directory.path().join("calls")).unwrap(),
            "x"
        );
        assert!(
            get_subtitles(Some(&service), page_input(offset, Some("0".repeat(64))))
                .await
                .is_err()
        );
        assert!(
            get_subtitles(Some(&service), page_input(usize::MAX, version))
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_provider_never_returns_or_caches_partial_files() {
        let (directory, service) =
            fixture_provider("WEBVTT\n\n00:00.000 --> 00:01.000\nНеполный текст\n", 1);
        let error = get_subtitles(Some(&service), page_input(0, None))
            .await
            .unwrap_err();
        assert!(error.message.contains("partial files"));
        assert!(service.cached("abcdefghijk").await.is_none());
        assert_eq!(
            std::fs::read_to_string(directory.path().join("calls")).unwrap(),
            "x"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tries_native_languages_in_order_and_stops_after_the_first_success() {
        let (directory, service) = fixture_provider(
            "WEBVTT\n\n00:00.000 --> 00:01.000\nOriginal English track\n",
            0,
        );
        std::fs::write(
            &service.config.command,
            r##"#!/bin/sh
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; output="$1" ;;
    --sub-langs) shift; language="$1" ;;
    --extractor-args) shift; extractor="$1" ;;
  esac
  shift
done
test "$extractor" = youtube:skip=translated_subs || exit 2
printf '%s\n' "$language" >> "$root/calls"
if [ "$language" = en ]; then
  cp "$root/source.vtt" "$(dirname -- "$output")/fixture.en.vtt"
fi
"##,
        )
        .unwrap();
        let output = get_subtitles(Some(&service), page_input(0, None))
            .await
            .unwrap();
        assert_eq!(output.videos[0].language.as_deref(), Some("en"));
        assert_eq!(
            output.videos[0].transcript.as_deref(),
            Some("Original English track")
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("calls")).unwrap(),
            "ru\nen\n"
        );
        let _ = get_subtitles(Some(&service), page_input(0, None))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(directory.path().join("calls")).unwrap(),
            "ru\nen\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn validates_the_whole_batch_before_any_download_and_bounds_concurrency() {
        let (directory, service) = fixture_provider("WEBVTT", 0);
        let mut input = page_input(0, None);
        input.urls.push("https://127.0.0.1/private".to_owned());
        assert!(get_subtitles(Some(&service), input).await.is_err());
        assert!(!directory.path().join("calls").exists());
        let _permit = service.fetch_permit.acquire().await.unwrap();
        let error = get_subtitles(Some(&service), page_input(0, None))
            .await
            .unwrap_err();
        assert!(error.message.contains("busy"));
        assert!(!directory.path().join("calls").exists());
    }

    #[tokio::test(start_paused = true)]
    async fn cache_survives_thirteen_minutes_but_expires_after_one_hour() {
        let service = YoutubeSubtitlesService::new(YoutubeSubtitlesConfig {
            command: "/not/invoked".into(),
            languages: "ru".into(),
            timeout: Duration::from_secs(1),
            max_chars: 12000,
            max_videos: 1,
        });
        service.cache.lock().await.push_back(CachedTranscript {
            video_id: "abcdefghijk".into(),
            loaded_at: Instant::now(),
            transcript: Arc::new(Transcript {
                text: "Полный текст".into(),
                language: "ru".into(),
                id: "a".repeat(64),
                subtitle_end_seconds: Some(9000.0),
            }),
        });
        tokio::time::advance(Duration::from_secs(14 * 60)).await;
        assert!(service.cached("abcdefghijk").await.is_some());
        tokio::time::advance(CACHE_TTL).await;
        assert!(service.cached("abcdefghijk").await.is_none());
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
            Some(&YoutubeSubtitlesService::new(config)),
            GetSubtitlesInput {
                urls: vec!["https://example.com/watch?v=abcdefghijk".to_owned()],
                offset: 0,
                max_chars: None,
                transcript_id: None,
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
