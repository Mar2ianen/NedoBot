use super::*;
use sha2::{Digest, Sha256};
use teloxide_url::{
    Found, Kind, Source, TelegramLink, TelegramPeer, parse_telegram_link, parse_url,
};

pub(super) struct EvidenceRow {
    pub(super) index: i16,
    pub(super) kind: &'static str,
    pub(super) source: &'static str,
    pub(super) target_sha256: String,
    pub(super) host: Option<String>,
    pub(super) display_hint: String,
    pub(super) privacy_sensitive: bool,
    pub(super) qr_relation: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrUrlRelation {
    Affiliated,
    External,
    Unresolved,
}

impl QrUrlRelation {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Affiliated => "affiliated",
            Self::External => "external",
            Self::Unresolved => "unresolved",
        }
    }
}

#[derive(Debug, Clone)]
pub struct QrUrlEvidence {
    pub target: String,
    pub relation: QrUrlRelation,
}

pub(super) fn encode(
    found: &Found,
    index: usize,
    qr_relation: Option<&'static str>,
) -> anyhow::Result<EvidenceRow> {
    let kind = match found.kind {
        Kind::Url => "url",
        Kind::Mention => "mention",
        Kind::UserId(_) => "user_id",
        Kind::Hashtag => "hashtag",
        Kind::Cashtag => "cashtag",
        Kind::BotCommand => "bot_command",
    };
    let source = source_name(found.source);
    let parsed = (found.kind == Kind::Url)
        .then(|| parse_url(&found.value))
        .flatten();
    let normalized = match found.kind {
        Kind::UserId(id) => format!("telegram-user-id:{id}"),
        _ => parsed
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| found.value.trim().to_lowercase()),
    };
    let mut digest = Sha256::new();
    digest.update(b"nedobot-url-evidence-v1\0");
    digest.update(kind.as_bytes());
    digest.update(b"\0");
    digest.update(normalized.as_bytes());
    let target_sha256 = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    let host = parsed
        .as_ref()
        .and_then(|url| url.host_str())
        .map(str::to_owned);
    let display_hint = parsed.as_ref().map_or_else(
        || format!("{kind}:redacted"),
        |url| match url.host_str() {
            Some(host) => format!("{}://{host}/…", url.scheme()),
            None => format!("{}://…", url.scheme()),
        },
    );
    let telegram_sensitive = parse_telegram_link(&found.value).is_some_and(|link| {
        matches!(
            link,
            TelegramLink::Invite { .. }
                | TelegramLink::AmbiguousPlus { .. }
                | TelegramLink::BotStart { .. }
                | TelegramLink::Share { .. }
                | TelegramLink::Reserved { .. }
                | TelegramLink::Message {
                    peer: TelegramPeer::ChatId(_),
                    ..
                }
        )
    });
    let url_sensitive = parsed
        .as_ref()
        .is_some_and(|url| url.path() != "/" || url.query().is_some());

    Ok(EvidenceRow {
        index: i16::try_from(index)?,
        kind,
        source,
        target_sha256,
        host,
        display_hint,
        privacy_sensitive: telegram_sensitive || url_sensitive,
        qr_relation,
    })
}

pub(super) fn source_name(source: Source) -> &'static str {
    match source {
        Source::VisibleText => "visible_text",
        Source::TelegramEntity => "telegram_entity",
        Source::RichMessage => "rich_message",
        Source::Qr => "qr",
    }
}

pub(super) fn canonical_url_identity(value: &str) -> String {
    teloxide_url::parse_url(value)
        .map(|url| url.to_string())
        .unwrap_or_else(|| value.to_owned())
}

pub(super) fn source_fingerprint(
    message: &Message,
    qr_hashes: &[String],
) -> anyhow::Result<String> {
    let bytes = serde_json::to_vec(message)?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    for hash in qr_hashes {
        digest.update(b"\0qr:");
        digest.update(hash.as_bytes());
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod target_identity_tests {
    use teloxide_url::{Found, Kind, Source};

    use super::encode;

    #[test]
    fn native_user_mentions_hash_the_telegram_id_not_the_visible_label() {
        let alice_42 = encode(&Found::new("Alice", Kind::UserId(42)), 0, None).unwrap();
        let alice_43 = encode(&Found::new("Alice", Kind::UserId(43)), 0, None).unwrap();
        let different_label_same_id = encode(&Found::new("A.", Kind::UserId(42)), 0, None).unwrap();

        assert_ne!(alice_42.target_sha256, alice_43.target_sha256);
        assert_eq!(
            alice_42.target_sha256,
            different_label_same_id.target_sha256
        );
    }

    #[test]
    fn same_canonical_url_has_one_identity_across_all_extraction_sources() {
        let sources = [
            Source::VisibleText,
            Source::TelegramEntity,
            Source::RichMessage,
            Source::Qr,
        ];
        let hashes = sources.map(|source| {
            let mut found = Found::new("https://example.org/a", Kind::Url);
            found.source = source;
            encode(&found, 0, (source == Source::Qr).then_some("external"))
                .unwrap()
                .target_sha256
        });
        assert!(hashes.iter().all(|hash| hash == &hashes[0]));
        let different = encode(&Found::new("https://example.org/b", Kind::Url), 0, None).unwrap();
        assert_ne!(hashes[0], different.target_sha256);

        let unicode_sources = [
            Source::VisibleText,
            Source::TelegramEntity,
            Source::RichMessage,
        ]
        .map(|source| {
            let mut found = Found::new("https://пример.рф/Путь?Ключ=Знач", Kind::Url);
            found.source = source;
            encode(&found, 0, None).unwrap().target_sha256
        });
        assert!(
            unicode_sources
                .iter()
                .all(|hash| hash == &unicode_sources[0])
        );
    }
}
