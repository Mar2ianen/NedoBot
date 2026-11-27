use std::{
    collections::HashMap,
    sync::OnceLock,
    time::{Duration, Instant},
};

#[cfg(feature = "emoji")]
use teloxide::errors::RequestError;
#[cfg(feature = "emoji")]
use teloxide::types::{MessageEntity, MessageEntityKind};
use teloxide::{
    adaptors::DefaultParseMode,
    prelude::{Bot, Message, Requester, ResponseResult},
    types::CustomEmojiId,
};

use crate::telegram::{
    entities::custom_emoji_ids,
    render::escape_html,
    service_messages::{self, MessageAudience},
};

const MAX_CACHE_ENTRIES: usize = 512;
const MAX_LOOKUP_IDS_PER_REQUEST: usize = 200;
const POSITIVE_TTL: Duration = Duration::from_secs(86_400);
const MISSING_TTL: Duration = Duration::from_secs(600);
const ERROR_TTL: Duration = Duration::from_secs(30);
#[cfg(feature = "emoji")]
const MAX_COMMAND_LOOKUPS: usize = 200;
#[cfg(feature = "emoji")]
const MAX_OUTPUT_ENTITIES: usize = 100;
#[cfg(feature = "emoji")]
const MAX_OUTPUT_UTF16: usize = 3500;

#[derive(Clone)]
struct CachedEmoji {
    expires_at: Instant,
    fallback: Option<String>,
}

type EmojiCache = tokio::sync::Mutex<HashMap<String, CachedEmoji>>;

fn cache() -> &'static EmojiCache {
    static CACHE: OnceLock<EmojiCache> = OnceLock::new();
    CACHE.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

/// Resolve metadata through Bot API, caching positive, confirmed missing, and
/// transient-error results separately by their TTL. The cache lock is never
/// held across a network await. IDs are deduplicated for lookup only.
pub async fn resolve_custom_emoji_fallbacks(
    bot: &DefaultParseMode<Bot>,
    ids: &[String],
) -> HashMap<String, String> {
    let unique = ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let now = Instant::now();
    let mut unresolved = Vec::new();
    {
        let mut cached = cache().lock().await;
        cached.retain(|_, value| value.expires_at > now);
        for id in unique {
            if !cached.contains_key(&id) {
                unresolved.push(id);
            }
        }
    }

    for batch in unresolved.chunks(MAX_LOOKUP_IDS_PER_REQUEST) {
        let stickers = bot
            .get_custom_emoji_stickers(batch.iter().cloned().map(CustomEmojiId))
            .await;
        let mut values = HashMap::new();
        let (ttl, fallback_on_error) = match stickers {
            Ok(stickers) => {
                for sticker in stickers {
                    let Some(id) = sticker.custom_emoji_id().map(|id| id.0.clone()) else {
                        continue;
                    };
                    if batch.iter().any(|requested| requested == &id) {
                        let emoji = sticker.emoji.filter(|emoji| valid_canonical_emoji(emoji));
                        values.insert(id, emoji);
                    }
                }
                (MISSING_TTL, false)
            }
            Err(error) => {
                tracing::warn!(%error, "failed to resolve custom emoji metadata");
                (ERROR_TTL, true)
            }
        };
        let now = Instant::now();
        let mut cached = cache().lock().await;
        cached.retain(|_, value| value.expires_at > now);
        for id in batch {
            let fallback = if fallback_on_error {
                None
            } else {
                values.remove(id).flatten()
            };
            let entry_ttl = if fallback_on_error {
                ERROR_TTL
            } else if fallback.is_some() {
                POSITIVE_TTL
            } else {
                ttl
            };
            cache_metadata_result(&mut cached, id, fallback, entry_ttl, now);
        }
    }

    let now = Instant::now();
    let cached = cache().lock().await;
    ids.iter()
        .filter_map(|id| {
            cached
                .get(id)
                .filter(|value| value.expires_at > now)
                .and_then(|value| value.fallback.clone())
                .map(|fallback| (id.clone(), fallback))
        })
        .collect()
}

fn cache_metadata_result(
    cache: &mut HashMap<String, CachedEmoji>,
    id: &str,
    fallback: Option<String>,
    ttl: Duration,
    now: Instant,
) {
    if fallback.is_none()
        && cache
            .get(id)
            .is_some_and(|entry| entry.expires_at > now && entry.fallback.is_some())
    {
        // A concurrent slow negative/error lookup must not erase an already
        // confirmed fallback. A positive result below always wins.
        return;
    }
    if cache.len() >= MAX_CACHE_ENTRIES
        && !cache.contains_key(id)
        && let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, value)| value.expires_at)
            .map(|(key, _)| key.clone())
    {
        cache.remove(&oldest);
    }
    cache.insert(
        id.to_owned(),
        CachedEmoji {
            expires_at: now + ttl,
            fallback,
        },
    );
}

fn valid_canonical_emoji(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
        return false;
    }
    #[cfg(feature = "emoji")]
    {
        use unicode_segmentation::UnicodeSegmentation;
        value.graphemes(true).count() == 1 && emojis::get(value).is_some()
    }
    #[cfg(not(feature = "emoji"))]
    {
        value.chars().count() <= 16
    }
}

pub async fn send_custom_emoji_ids(
    bot: &DefaultParseMode<Bot>,
    msg: &Message,
    audience: Option<MessageAudience>,
) -> ResponseResult<()> {
    let Some(audience) = audience else {
        return Ok(());
    };
    let target = custom_emoji_ids_target(msg);
    let ids = match custom_emoji_ids(target) {
        Ok(ids) => ids,
        Err(reason) => {
            service_messages::send_html(
                bot,
                msg.chat.id,
                format!(
                    "Не могу безопасно разобрать emoji в сообщении: <code>{}</code>",
                    escape_html(&reason)
                ),
                audience,
            )
            .await?;
            return Ok(());
        }
    };
    if ids.is_empty() {
        service_messages::send_html(
            bot,
            msg.chat.id,
            "В этом сообщении нет premium/custom emoji entities.",
            audience,
        )
        .await?;
        return Ok(());
    }

    let lines = ids
        .iter()
        .map(|id| format!("<code>{}</code>", escape_html(id)))
        .collect::<Vec<_>>()
        .join("\n");
    service_messages::send_html(
        bot,
        msg.chat.id,
        format!("Нашёл custom_emoji_id:\n{}", lines),
        audience,
    )
    .await?;
    Ok(())
}

fn custom_emoji_ids_target(command: &Message) -> &Message {
    command.reply_to_message().unwrap_or(command)
}

#[cfg(feature = "emoji")]
pub async fn send_custom_emojis(
    bot: &DefaultParseMode<Bot>,
    command: &Message,
    audience: Option<MessageAudience>,
) -> ResponseResult<()> {
    let Some(audience) = audience else {
        return Ok(());
    };
    let Some(target) = command.reply_to_message() else {
        service_messages::send_html(
            bot,
            command.chat.id,
            "Ответьте командой /emoji на сообщение с custom emoji.",
            audience,
        )
        .await?;
        return Ok(());
    };
    let occurrences = match crate::features::emoji_occurrences::extract_message(target) {
        Ok(occurrences) => occurrences,
        Err(error) => {
            service_messages::send_html(
                bot,
                command.chat.id,
                format!("Не могу безопасно разобрать сообщение: <code>{error:?}</code>"),
                audience,
            )
            .await?;
            return Ok(());
        }
    };
    let custom = occurrences
        .into_iter()
        .filter_map(|occurrence| match occurrence.identity.clone() {
            crate::features::emoji_analysis::EmojiIdentity::Custom(id) => Some((id, occurrence)),
            crate::features::emoji_analysis::EmojiIdentity::Unicode(_) => None,
        })
        .collect::<Vec<_>>();
    if custom.is_empty() {
        service_messages::send_html(
            bot,
            command.chat.id,
            "В reply нет custom/premium emoji. Обычные Unicode emoji не отправляю.",
            audience,
        )
        .await?;
        return Ok(());
    }

    let ids = custom.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
    let unique = ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let lookup_overflow = unique.len() > MAX_COMMAND_LOOKUPS;
    let lookup_ids = unique
        .into_iter()
        .take(MAX_COMMAND_LOOKUPS)
        .collect::<Vec<_>>();
    let metadata = resolve_custom_emoji_fallbacks(bot, &lookup_ids).await;
    let (text, entities, unresolved, truncated) = build_custom_emoji_message(&custom, &metadata);
    if text.is_empty() {
        let ids = if unresolved.is_empty() {
            lookup_ids
        } else {
            unresolved
        };
        let list = escape_html(&display_ids(&ids));
        service_messages::send_html(bot, command.chat.id, format!("Не удалось получить metadata для custom emoji ID: <code>{list}</code>. Bot API не отправил их как premium emoji."), audience).await?;
        return Ok(());
    }
    let suffix = if unresolved.is_empty() && !lookup_overflow && !truncated {
        String::new()
    } else {
        let mut notes = Vec::new();
        if !unresolved.is_empty() {
            notes.push(format!(
                "Не удалось разрешить ID: {}",
                display_ids(&unresolved)
            ));
        }
        if lookup_overflow {
            notes.push("Часть ID пропущена из-за лимита lookup.".to_owned());
        }
        if truncated {
            notes.push("Вывод обрезан по лимиту Telegram.".to_owned());
        }
        format!("\n\nПримечание: {}", notes.join(" "))
    };
    let expected_custom = entities.len();
    let mut expected_ids = entities
        .iter()
        .filter_map(|entity| match &entity.kind {
            MessageEntityKind::CustomEmoji { custom_emoji_id } => Some(custom_emoji_id.0.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    expected_ids.sort();
    let sent = match service_messages::send_entities(
        bot,
        command,
        &format!("{text}{suffix}"),
        entities,
        audience,
    )
    .await
    {
        Ok(sent) => sent,
        Err(RequestError::Api(error)) => {
            let list = escape_html(&display_ids(&lookup_ids));
            service_messages::send_html(
                bot,
                command.chat.id,
                format!("Telegram отклонил отправку custom emoji; проверьте доступность для бота/чата. ID: <code>{list}</code>. Unicode fallback не отправлялся."),
                audience,
            )
            .await?;
            tracing::warn!(%error, chat_id = command.chat.id.0, "Telegram rejected /emoji custom entity delivery");
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let mut returned_ids = sent
        .entities()
        .unwrap_or_default()
        .iter()
        .filter_map(|entity| match &entity.kind {
            MessageEntityKind::CustomEmoji { custom_emoji_id } => Some(custom_emoji_id.0.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    returned_ids.sort();
    if returned_ids.len() != expected_custom || returned_ids != expected_ids {
        tracing::error!(
            chat_id = command.chat.id.0,
            "Telegram acknowledged /emoji without custom emoji entities"
        );
        service_messages::send_html(bot, command.chat.id, "Telegram принял текст, но не подтвердил custom emoji entities. Unicode fallback не считается успешной отправкой.", audience).await?;
    }
    Ok(())
}

#[cfg(feature = "emoji")]
fn display_ids(ids: &[String]) -> String {
    ids.iter()
        .take(8)
        .map(|id| {
            let mut display = String::new();
            for ch in id.chars().take(48) {
                match ch {
                    '<' => display.push('‹'),
                    '>' => display.push('›'),
                    '&' => display.push('＆'),
                    ch if ch.is_control() => {
                        use std::fmt::Write;
                        let _ = write!(display, "\\u{{{:04X}}}", ch as u32);
                    }
                    ch => display.push(ch),
                }
            }
            if id.chars().count() > 48 {
                display.push('…');
            }
            display
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(feature = "emoji")]
fn build_custom_emoji_message(
    occurrences: &[(String, crate::features::emoji_occurrences::EmojiOccurrence)],
    metadata: &HashMap<String, String>,
) -> (String, Vec<MessageEntity>, Vec<String>, bool) {
    let mut text = String::new();
    let mut entities = Vec::new();
    let mut unresolved = Vec::new();
    let mut seen_unresolved = std::collections::HashSet::new();
    let mut truncated = false;
    for (id, _) in occurrences {
        let Some(label) = metadata
            .get(id)
            .filter(|label| valid_canonical_emoji(label))
        else {
            if seen_unresolved.insert(id.clone()) {
                unresolved.push(id.clone());
            }
            continue;
        };
        if entities.len() >= MAX_OUTPUT_ENTITIES {
            truncated = true;
            break;
        }
        let offset = text
            .encode_utf16()
            .count()
            .saturating_add(usize::from(!text.is_empty()));
        let length = label.encode_utf16().count();
        if offset.saturating_add(length) > MAX_OUTPUT_UTF16 {
            truncated = true;
            break;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(label);
        entities.push(MessageEntity::custom_emoji(
            CustomEmojiId(id.clone()),
            offset,
            length,
        ));
    }
    (text, entities, unresolved, truncated)
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "emoji")]
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::{TcpListener, TcpStream},
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use teloxide::types::Message;
    #[cfg(feature = "emoji")]
    use teloxide::{prelude::RequesterExt, types::ParseMode};

    #[test]
    fn emoji_ids_prefers_the_replied_message() {
        let command: Message = serde_json::from_value(serde_json::json!({
            "message_id": 10,
            "date": 1,
            "chat": {"id": -1001, "type": "supergroup", "title": "Test"},
            "text": "/emoji_ids",
            "reply_to_message": {
                "message_id": 9,
                "date": 1,
                "chat": {"id": -1001, "type": "supergroup", "title": "Test"},
                "text": "😀"
            }
        }))
        .expect("command with reply");
        assert_eq!(super::custom_emoji_ids_target(&command).id.0, 9);
    }

    #[cfg(feature = "emoji")]
    #[test]
    fn output_entities_use_utf16_offsets_and_preserve_repeated_ids() {
        let occurrences = vec![
            ("one".into(), occurrence()),
            ("one".into(), occurrence()),
            ("two".into(), occurrence()),
        ];
        let metadata = [("one".into(), "🤦🏽‍♀️".into()), ("two".into(), "😎".into())]
            .into_iter()
            .collect();
        let (text, entities, unresolved, truncated) =
            super::build_custom_emoji_message(&occurrences, &metadata);
        assert_eq!(text, "🤦🏽‍♀️ 🤦🏽‍♀️ 😎");
        assert!(unresolved.is_empty());
        assert!(!truncated);
        assert_eq!(entities.len(), 3);
        assert_eq!((entities[0].offset, entities[0].length), (0, 7));
        assert_eq!((entities[1].offset, entities[1].length), (8, 7));
        assert_eq!((entities[2].offset, entities[2].length), (16, 2));
    }

    #[cfg(feature = "emoji")]
    #[test]
    fn invalid_metadata_is_not_sent_as_a_unicode_success() {
        let occurrences = vec![("broken".into(), occurrence())];
        let metadata = [("broken".into(), "not emoji".into())]
            .into_iter()
            .collect();
        let (text, entities, unresolved, _) =
            super::build_custom_emoji_message(&occurrences, &metadata);
        assert!(text.is_empty());
        assert!(entities.is_empty());
        assert_eq!(unresolved, ["broken"]);
    }

    #[test]
    fn late_negative_cache_write_preserves_positive_but_positive_replaces_negative() {
        let now = std::time::Instant::now();
        let mut cache = std::collections::HashMap::new();
        super::cache_metadata_result(
            &mut cache,
            "race-id",
            None,
            std::time::Duration::from_secs(30),
            now,
        );
        super::cache_metadata_result(
            &mut cache,
            "race-id",
            Some("🤖".to_owned()),
            std::time::Duration::from_secs(100),
            now,
        );
        super::cache_metadata_result(
            &mut cache,
            "race-id",
            None,
            std::time::Duration::from_secs(30),
            now,
        );
        assert_eq!(
            cache
                .get("race-id")
                .and_then(|entry| entry.fallback.as_deref()),
            Some("🤖")
        );

        super::cache_metadata_result(
            &mut cache,
            "race-id",
            Some("😎".to_owned()),
            std::time::Duration::from_secs(100),
            now,
        );
        assert_eq!(
            cache
                .get("race-id")
                .and_then(|entry| entry.fallback.as_deref()),
            Some("😎")
        );
    }

    #[cfg(feature = "emoji")]
    #[tokio::test]
    async fn resolver_batches_reorders_partial_results_and_caches_missing_ids() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock Bot API");
        let address = listener.local_addr().expect("mock Bot API address");
        let server = thread::spawn(move || {
            let mut request_count = 0;
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept batch");
                request_count += serve_emoji_lookup(&mut stream, true);
            }
            listener
                .set_nonblocking(true)
                .expect("observe unexpected cache misses");
            let deadline = std::time::Instant::now() + Duration::from_millis(250);
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        request_count += serve_emoji_lookup(&mut stream, false);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("listener failed: {error}"),
                }
            }
            request_count
        });
        let bot = teloxide::Bot::new("fixture")
            .set_api_url(format!("http://{address}/").parse().expect("mock URL"))
            .parse_mode(ParseMode::Html);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let ids = (0..201)
            .map(|index| format!("resolver-{nonce}-{index:03}"))
            .collect::<Vec<_>>();

        let first = super::resolve_custom_emoji_fallbacks(&bot, &ids).await;
        assert_eq!(first.len(), 200, "one confirmed missing ID stays absent");
        assert_eq!(first.get(&ids[1]).map(String::as_str), Some("🤖"));
        assert_eq!(first.get(&ids[2]).map(String::as_str), Some("😎"));
        assert!(!first.contains_key(&ids[0]));
        let second = super::resolve_custom_emoji_fallbacks(&bot, &ids).await;
        assert_eq!(second, first);
        assert_eq!(server.join().expect("mock API server finishes"), 2);
    }

    #[cfg(feature = "emoji")]
    fn serve_emoji_lookup(stream: &mut TcpStream, omit_first: bool) -> usize {
        let payload = read_json_request(stream);
        let ids = payload["custom_emoji_ids"]
            .as_array()
            .expect("getCustomEmojiStickers IDs")
            .iter()
            .map(|id| id.as_str().expect("string custom emoji ID").to_owned())
            .collect::<Vec<_>>();
        assert!(ids.len() <= super::MAX_LOOKUP_IDS_PER_REQUEST);
        let stickers = ids
            .iter()
            .rev()
            .filter(|id| !(omit_first && id.ends_with("-000")))
            .map(|id| {
                serde_json::json!({
                    "file_id": format!("file-{id}"),
                    "file_unique_id": format!("unique-{id}"),
                    "width": 512,
                    "height": 512,
                    "type": "custom_emoji",
                    "custom_emoji_id": id,
                    "is_animated": false,
                    "is_video": false,
                    "emoji": if id.ends_with("-001") { "🤖" } else { "😎" }
                })
            })
            .collect::<Vec<_>>();
        let body = serde_json::json!({"ok":true,"result":stickers}).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write fake response");
        stream.flush().expect("flush fake response");
        1
    }

    #[cfg(feature = "emoji")]
    fn read_json_request(stream: &TcpStream) -> serde_json::Value {
        let mut reader = BufReader::new(stream.try_clone().expect("clone request stream"));
        let mut content_length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("read request header");
            if line == "\r\n" || line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                content_length = value.trim().parse().expect("content length");
            }
        }
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).expect("read request body");
        serde_json::from_slice(&body).expect("JSON request")
    }

    #[cfg(feature = "emoji")]
    fn occurrence() -> crate::features::emoji_occurrences::EmojiOccurrence {
        crate::features::emoji_occurrences::EmojiOccurrence {
            identity: crate::features::emoji_analysis::EmojiIdentity::Custom("id".into()),
            fallback: "😀".into(),
            source: crate::features::emoji_occurrences::ContentSource::Text,
            path: "text".into(),
            utf8_range: 0..4,
            utf16_range: 0..2,
            format: crate::features::emoji_analysis::EmojiFormat::Unknown,
            encoding: crate::features::emoji_analysis::EmojiEncoding::NativeCustom,
            has_custom_url_fallback: false,
        }
    }
}
