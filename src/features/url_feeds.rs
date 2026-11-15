//! Локальный exact-URL feed; снимок opt-in, а его score остаётся shadow-only.
use crate::config::Config;
use chrono::DateTime;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::HashSet, sync::Arc, time::Duration};
use teloxide_url::parse_url;
use tokio::sync::RwLock;

const MAX_LINES: usize = 1_000_000;
const MAX_LINE_BYTES: usize = 4096;
const MAX_DOWNLOAD_BYTES: usize = 96 * 1024 * 1024;
const MAX_METADATA_BYTES: usize = 2 * 1024 * 1024;
const MAX_FEED_LINES: usize = 2_000_000;
const MIN_VALID_FEED_ENTRIES: usize = 1_000;
const MAX_INVALID_FEED_RATIO_PERCENT: usize = 40;
const FEED_URL_PREFIX: &str =
    "https://raw.githubusercontent.com/Phishing-Database/Phishing.Database";
const FEED_COMMIT_URL: &str = "https://api.github.com/repos/Phishing-Database/Phishing.Database/commits?path=phishing-links-ACTIVE.txt&per_page=1";
const FEED_LICENSE: &str = "MIT";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchScope {
    ExactUrl,
    Host,
    HostAndSubdomains,
}

#[derive(Debug, Clone)]
pub struct FeedSnapshot {
    pub source: String,
    pub license: String,
    pub revision: String,
    pub loaded_unix: u64,
    pub max_age_seconds: u64,
    pub scope: MatchScope,
    entries: HashSet<String>,
    evidence_hashes: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedHit<'a> {
    pub source: &'a str,
    pub revision: &'a str,
    pub scope: MatchScope,
}

impl FeedSnapshot {
    /// Форматы feed обрабатываются отдельным импортёром с проверенной лицензией.
    /// Здесь принимаются разобранные строки URL/host, а не правила ABP/uBO.
    pub fn from_lines(
        source: impl Into<String>,
        license: impl Into<String>,
        revision: impl Into<String>,
        loaded_unix: u64,
        max_age_seconds: u64,
        scope: MatchScope,
        raw: &str,
    ) -> Result<Self, &'static str> {
        let mut entries = HashSet::new();
        let mut evidence_hashes = HashSet::new();
        if raw.len() > 64 * 1024 * 1024 {
            return Err("feed too large");
        }
        for (index, line) in raw.lines().enumerate() {
            if index >= MAX_LINES {
                return Err("too many entries");
            }
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.len() > MAX_LINE_BYTES {
                return Err("long entry");
            }
            let item = match scope {
                MatchScope::ExactUrl => {
                    let url = parse_url(line).ok_or("invalid exact URL")?;
                    if !matches!(url.scheme(), "http" | "https") {
                        return Err("unsupported URL scheme");
                    }
                    url.to_string()
                }
                MatchScope::Host | MatchScope::HostAndSubdomains => {
                    if line
                        .chars()
                        .any(|c| matches!(c, '/' | ':' | '@' | '?' | '#' | '*'))
                    {
                        return Err("invalid host entry");
                    }
                    let url = parse_url(&format!("https://{line}/")).ok_or("invalid host")?;
                    let host = url.host_str().ok_or("missing host")?;
                    // Не допускаем случайного добавления общего хоста как шаблона.
                    if host.is_empty() || host == "t.me" {
                        return Err("prohibited broad host");
                    }
                    host.to_owned()
                }
            };
            if scope == MatchScope::ExactUrl {
                evidence_hashes.insert(url_evidence_hash(&item));
            }
            entries.insert(item);
        }
        Ok(Self {
            source: source.into(),
            license: license.into(),
            revision: revision.into(),
            loaded_unix,
            max_age_seconds,
            scope,
            entries,
            evidence_hashes,
        })
    }

    /// Parses the fixed upstream URL list conservatively. Non-HTTP entries
    /// and malformed rows are skipped, while size, line count, line length,
    /// invalid-ratio, and minimum-entry bounds fail the whole snapshot.
    fn from_upstream_lines(
        revision: String,
        upstream_unix: u64,
        max_age_seconds: u64,
        raw: &str,
    ) -> Result<Self, &'static str> {
        if raw.len() > MAX_DOWNLOAD_BYTES {
            return Err("feed too large");
        }
        let mut entries = HashSet::new();
        let mut evidence_hashes = HashSet::new();
        let mut considered = 0usize;
        let mut data_lines = 0usize;
        let mut invalid = 0usize;
        for line in raw.lines() {
            considered += 1;
            if considered > MAX_FEED_LINES {
                return Err("too many feed lines");
            }
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            data_lines += 1;
            if line.len() > MAX_LINE_BYTES {
                invalid += 1;
                continue;
            }
            let Some(url) = parse_url(line) else {
                invalid += 1;
                continue;
            };
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none_or(str::is_empty)
                || !url.username().is_empty()
                || url.password().is_some()
            {
                invalid += 1;
                continue;
            }
            let normalized = url.to_string();
            evidence_hashes.insert(url_evidence_hash(&normalized));
            entries.insert(normalized);
        }
        if entries.len() < MIN_VALID_FEED_ENTRIES {
            return Err("too few valid feed entries");
        }
        if data_lines > 0
            && invalid.saturating_mul(100)
                > data_lines.saturating_mul(MAX_INVALID_FEED_RATIO_PERCENT)
        {
            return Err("too many invalid feed entries");
        }
        Ok(Self {
            source: "Phishing.Database/phishing-links-ACTIVE.txt".to_owned(),
            license: FEED_LICENSE.to_owned(),
            revision,
            loaded_unix: upstream_unix,
            max_age_seconds,
            scope: MatchScope::ExactUrl,
            entries,
            evidence_hashes,
        })
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn is_fresh(&self, now_unix: u64) -> bool {
        now_unix.saturating_sub(self.loaded_unix) <= self.max_age_seconds
    }

    pub fn contains_evidence_hash(&self, hash: &str, now_unix: u64) -> bool {
        self.is_fresh(now_unix) && self.evidence_hashes.contains(hash)
    }

    pub fn lookup(&self, raw: &str, now_unix: u64) -> Option<FeedHit<'_>> {
        if now_unix.saturating_sub(self.loaded_unix) > self.max_age_seconds {
            return None;
        }
        let parsed = parse_url(raw)?;
        let host = parsed.host_str()?;
        // Доменные списки нельзя применять к схемам вроде tg://.
        if !matches!(parsed.scheme(), "http" | "https") {
            return None;
        }
        let hit = match self.scope {
            MatchScope::ExactUrl => self.entries.contains(parsed.as_str()),
            MatchScope::Host => self.entries.contains(host),
            MatchScope::HostAndSubdomains => {
                self.entries.contains(host)
                    || host
                        .match_indices('.')
                        .any(|(i, _)| self.entries.contains(&host[i + 1..]))
            }
        };
        hit.then_some(FeedHit {
            source: &self.source,
            revision: &self.revision,
            scope: self.scope,
        })
    }
}

fn url_evidence_hash(normalized_url: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"nedobot-url-evidence-v1\0url\0");
    digest.update(normalized_url.as_bytes());
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Deserialize)]
struct FeedCommit {
    sha: String,
    commit: FeedCommitDetails,
}

#[derive(Debug, Deserialize)]
struct FeedCommitDetails {
    committer: FeedCommitter,
}

#[derive(Debug, Deserialize)]
struct FeedCommitter {
    date: String,
}

/// Starts an opt-in, bounded updater. The feed origin and metadata endpoint
/// are constants; no URL from Telegram is ever sent to either service.
pub fn spawn_worker(snapshot: Arc<RwLock<Option<Arc<FeedSnapshot>>>>, config: Config) {
    if !config.community.moderation.url_feed_enabled {
        return;
    }
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120))
            .user_agent(concat!("NedoBot/", env!("CARGO_PKG_VERSION")))
            .build()
        {
            Ok(client) => client,
            Err(_) => {
                tracing::error!("failed to create URL feed HTTP client");
                return;
            }
        };
        let refresh_seconds = config.community.moderation.url_feed_refresh_seconds;
        let max_age_seconds = config.community.moderation.url_feed_max_age_seconds;
        let mut interval = tokio::time::interval(Duration::from_secs(refresh_seconds));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match download_snapshot(&client, max_age_seconds).await {
                Ok(candidate) => {
                    let entry_count = candidate.entry_count();
                    let fresh = candidate.is_fresh(unix_now());
                    let revision = candidate.revision.clone();
                    *snapshot.write().await = Some(Arc::new(candidate));
                    tracing::info!(entry_count, %revision, fresh, "replaced local phishing URL feed snapshot");
                }
                Err(error) => {
                    tracing::warn!(%error, "URL feed refresh failed; previous snapshot retained");
                }
            }
        }
    });
}

async fn download_snapshot(
    client: &reqwest::Client,
    max_age_seconds: u64,
) -> anyhow::Result<FeedSnapshot> {
    let metadata_response = client
        .get(FEED_COMMIT_URL)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?;
    anyhow::ensure!(
        metadata_response.status().is_success(),
        "feed metadata endpoint returned a non-success status"
    );
    let mut metadata_response = metadata_response;
    let mut metadata_bytes = Vec::new();
    while let Some(chunk) = metadata_response.chunk().await? {
        anyhow::ensure!(
            metadata_bytes.len().saturating_add(chunk.len()) <= MAX_METADATA_BYTES,
            "feed metadata exceeded the configured size bound"
        );
        metadata_bytes.extend_from_slice(&chunk);
    }
    let metadata: Vec<FeedCommit> = serde_json::from_slice(&metadata_bytes)?;
    let metadata = metadata
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("feed commit metadata had no history"))?;
    anyhow::ensure!(
        metadata.sha.len() == 40 && metadata.sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "feed metadata had an invalid commit SHA"
    );
    let upstream_time = DateTime::parse_from_rfc3339(&metadata.commit.committer.date)?
        .timestamp()
        .max(0) as u64;

    let feed_url = format!(
        "{FEED_URL_PREFIX}/{}/phishing-links-ACTIVE.txt",
        metadata.sha.to_ascii_lowercase()
    );
    let response = client.get(feed_url).send().await?;
    anyhow::ensure!(
        response.status().is_success(),
        "feed endpoint returned a non-success status"
    );
    if let Some(length) = response.content_length() {
        anyhow::ensure!(
            length <= MAX_DOWNLOAD_BYTES as u64,
            "feed exceeded the configured download bound"
        );
    }
    let mut bytes = Vec::new();
    let mut response = response;
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= MAX_DOWNLOAD_BYTES,
            "feed exceeded the configured download bound"
        );
        bytes.extend_from_slice(&chunk);
    }
    let raw =
        std::str::from_utf8(&bytes).map_err(|_| anyhow::anyhow!("feed was not valid UTF-8"))?;
    FeedSnapshot::from_upstream_lines(metadata.sha, upstream_time, max_age_seconds, raw)
        .map_err(anyhow::Error::msg)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_exact_url_and_query() {
        let snap = FeedSnapshot::from_lines(
            "test",
            "CC0",
            "v1",
            100,
            3600,
            MatchScope::ExactUrl,
            "https://bad.example/path?token=abc",
        )
        .unwrap();
        assert!(
            snap.lookup("https://bad.example/path?token=abc", 101)
                .is_some()
        );
        assert!(
            snap.lookup("https://bad.example/path?token=def", 101)
                .is_none()
        );
        assert!(
            snap.lookup("https://bad.example/path?token=abc", 4000)
                .is_none()
        );
    }
    #[test]
    fn no_suffix_confusion() {
        let snap = FeedSnapshot::from_lines(
            "test",
            "CC0",
            "v1",
            100,
            3600,
            MatchScope::HostAndSubdomains,
            "bad.example",
        )
        .unwrap();
        assert!(snap.lookup("https://x.bad.example/", 101).is_some());
        assert!(snap.lookup("https://notbad.example/", 101).is_none());
    }

    #[test]
    fn upstream_snapshot_requires_enough_valid_entries_and_uses_commit_time() {
        let rows = (0..MIN_VALID_FEED_ENTRIES)
            .map(|index| format!("https://bad{index}.example/path"))
            .collect::<Vec<_>>()
            .join("\n");
        let snapshot = FeedSnapshot::from_upstream_lines("a".repeat(40), 100, 60, &rows).unwrap();
        assert_eq!(snapshot.entry_count(), MIN_VALID_FEED_ENTRIES);
        assert!(snapshot.is_fresh(160));
        assert!(!snapshot.is_fresh(161));
        assert!(snapshot.lookup("https://bad0.example/path", 150).is_some());
        assert!(
            snapshot.contains_evidence_hash(&url_evidence_hash("https://bad0.example/path"), 150)
        );
        assert!(snapshot.lookup("https://bad0.example/path", 161).is_none());
    }

    #[test]
    fn upstream_snapshot_skips_non_http_rows_but_rejects_bad_corpus() {
        let rows = (0..MIN_VALID_FEED_ENTRIES)
            .map(|index| format!("https://bad{index}.example/path"))
            .chain(std::iter::once("ftp://not-http.example/file".to_owned()))
            .collect::<Vec<_>>()
            .join("\n");
        let snapshot =
            FeedSnapshot::from_upstream_lines("revision".into(), 100, 60, &rows).unwrap();
        assert_eq!(snapshot.entry_count(), MIN_VALID_FEED_ENTRIES);
        assert!(
            FeedSnapshot::from_upstream_lines("revision".into(), 100, 60, "ftp://x.example/a")
                .is_err()
        );
    }
}
