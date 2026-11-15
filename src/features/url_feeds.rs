//! Локальное сопоставление со снимком feed; загрузчик, SQL-кеш и влияние на оценку отдельны.
//! Перед включением в production нужно проверить источники и лицензии на распространение.
use std::collections::HashSet;
use teloxide_url::parse_url;

const MAX_LINES: usize = 1_000_000;
const MAX_LINE_BYTES: usize = 4096;

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
        })
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
}
