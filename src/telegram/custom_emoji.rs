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
const MAX_CUSTOM_EMOJI_IDS_OUTPUT_UTF16: usize = 3_500;
const POSITIVE_TTL: Duration = Duration::from_secs(86_400);
const MISSING_TTL: Duration = Duration::from_secs(600);
const ERROR_TTL: Duration = Duration::from_secs(30);
const MAX_CONCURRENT_METADATA_LOOKUPS: usize = 4;
#[cfg(feature = "emoji")]
const MAX_COMMAND_LOOKUPS: usize = 200;
#[cfg(feature = "emoji")]
const MAX_OUTPUT_ENTITIES: usize = 100;
#[cfg(feature = "emoji")]
const MAX_OUTPUT_UTF16: usize = 2800;

#[derive(Clone)]
struct CachedEmoji {
    expires_at: Instant,
    state: MetadataLookupState,
}

#[derive(Clone)]
enum MetadataLookupState {
    Positive(String),
    ConfirmedMissing,
    TransientFailure,
}

type EmojiCache = tokio::sync::Mutex<HashMap<String, CachedEmoji>>;

fn lookup_slots() -> &'static tokio::sync::Semaphore {
    static SLOTS: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| tokio::sync::Semaphore::new(MAX_CONCURRENT_METADATA_LOOKUPS))
}

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
    let mut resolved = HashMap::new();
    {
        let mut cached = cache().lock().await;
        cached.retain(|_, value| value.expires_at > now);
        for id in unique {
            if let Some(fallback) = cached.get(&id).and_then(|value| match &value.state {
                MetadataLookupState::Positive(fallback) => Some(fallback.clone()),
                MetadataLookupState::ConfirmedMissing | MetadataLookupState::TransientFailure => {
                    None
                }
            }) {
                resolved.insert(id, fallback);
            } else if !cached.contains_key(&id) {
                unresolved.push(id);
            }
        }
    }

    let Ok(_lookup_slot) = lookup_slots().try_acquire() else {
        return resolved;
    };

    for batch in unresolved.chunks(MAX_LOOKUP_IDS_PER_REQUEST) {
        let stickers = bot
            .get_custom_emoji_stickers(batch.iter().cloned().map(CustomEmojiId))
            .await;
        let mut values = HashMap::new();
        let fallback_on_error = match stickers {
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
                false
            }
            Err(error) => {
                tracing::warn!(%error, "failed to resolve custom emoji metadata");
                true
            }
        };
        let now = Instant::now();
        let mut cached = cache().lock().await;
        cached.retain(|_, value| value.expires_at > now);
        for id in batch {
            let state = if fallback_on_error {
                MetadataLookupState::TransientFailure
            } else if let Some(fallback) = values.remove(id).flatten() {
                MetadataLookupState::Positive(fallback)
            } else {
                MetadataLookupState::ConfirmedMissing
            };
            cache_metadata_result(&mut cached, id, state, now);
            // Capture the result selected for this lookup while holding the
            // cache lock. A concurrent positive result wins over a late
            // missing/error response, and later eviction/expiry cannot erase
            // this call's own response.
            if let Some(fallback) = cached.get(id).and_then(|value| match &value.state {
                MetadataLookupState::Positive(fallback) => Some(fallback.clone()),
                MetadataLookupState::ConfirmedMissing | MetadataLookupState::TransientFailure => {
                    None
                }
            }) {
                resolved.insert(id.clone(), fallback);
            }
        }
    }
    resolved
}

fn cache_metadata_result(
    cache: &mut HashMap<String, CachedEmoji>,
    id: &str,
    state: MetadataLookupState,
    now: Instant,
) {
    if !matches!(&state, MetadataLookupState::Positive(_))
        && cache.get(id).is_some_and(|entry| {
            entry.expires_at > now && matches!(&entry.state, MetadataLookupState::Positive(_))
        })
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
    let ttl = match &state {
        MetadataLookupState::Positive(_) => POSITIVE_TTL,
        MetadataLookupState::ConfirmedMissing => MISSING_TTL,
        MetadataLookupState::TransientFailure => ERROR_TTL,
    };
    cache.insert(
        id.to_owned(),
        CachedEmoji {
            expires_at: now + ttl,
            state,
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
                    escape_html(&format!("{reason:?}"))
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

    service_messages::send_html(bot, msg.chat.id, format_custom_emoji_ids(&ids), audience).await?;
    Ok(())
}

fn format_custom_emoji_ids(ids: &[String]) -> String {
    let mut output = String::from("Нашёл custom_emoji_id:\n");
    let mut shown = 0;
    for id in ids {
        let mut display = id.chars().take(48).collect::<String>();
        if id.chars().count() > 48 {
            display.push('…');
        }
        let line = format!("<code>{}</code>\n", escape_html(&display));
        // Reserve using the current omitted count. If this candidate does not
        // fit, the final suffix keeps this count; if it fits, we merely reserve
        // one extra count character conservatively.
        let remaining = ids.len().saturating_sub(shown);
        let reserve = if remaining > 0 {
            format!("… и ещё {remaining} ID не показано")
        } else {
            String::new()
        };
        if output.encode_utf16().count()
            + line.encode_utf16().count()
            + reserve.encode_utf16().count()
            > MAX_CUSTOM_EMOJI_IDS_OUTPUT_UTF16
        {
            break;
        }
        output.push_str(&line);
        shown += 1;
    }
    if shown < ids.len() {
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&format!("… и ещё {} ID не показано", ids.len() - shown));
    }
    output
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
        .filter_map(|occurrence| match occurrence.kind {
            crate::features::emoji_analysis::EmojiKind::Custom { id, .. } => Some(id.0),
            crate::features::emoji_analysis::EmojiKind::Unicode { .. } => None,
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

    let (lookup_ids, lookup_overflow) = first_seen_bounded_ids(&custom, MAX_COMMAND_LOOKUPS);
    let metadata = resolve_custom_emoji_fallbacks(bot, &lookup_ids).await;
    let plan = build_custom_emoji_message(&custom, &metadata);
    if plan.text.is_empty() {
        let ids = if plan.unresolved.is_empty() {
            lookup_ids
        } else {
            plan.unresolved
        };
        let list = escape_html(&display_ids(&ids));
        service_messages::send_html(bot, command.chat.id, format!("Не удалось получить metadata для custom emoji ID: <code>{list}</code>. Bot API не отправил их как premium emoji."), audience).await?;
        return Ok(());
    }
    let suffix = if plan.unresolved.is_empty() && !lookup_overflow && !plan.truncated {
        String::new()
    } else {
        let mut notes = Vec::new();
        if !plan.unresolved.is_empty() {
            notes.push(format!(
                "Не удалось разрешить ID: {}",
                display_ids(&plan.unresolved)
            ));
        }
        if lookup_overflow {
            notes.push("Часть ID пропущена из-за лимита lookup.".to_owned());
        }
        if plan.truncated {
            notes.push("Вывод обрезан по лимиту Telegram.".to_owned());
        }
        format!("\n\nПримечание: {}", notes.join(" "))
    };
    let expected_custom = plan.entities.len();
    let mut expected_entities = plan
        .entities
        .iter()
        .filter_map(|entity| match &entity.kind {
            MessageEntityKind::CustomEmoji { custom_emoji_id } => {
                Some((entity.offset, entity.length, custom_emoji_id.0.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    expected_entities.sort();
    let expected_text = format!("{}{suffix}", plan.text);
    let sent = match service_messages::send_entities(
        bot,
        command,
        &expected_text,
        plan.entities,
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
    let sent_entities = sent.entities().unwrap_or_default();
    let mut returned_entities = sent_entities
        .iter()
        .filter_map(|entity| match &entity.kind {
            MessageEntityKind::CustomEmoji { custom_emoji_id } => {
                Some((entity.offset, entity.length, custom_emoji_id.0.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    returned_entities.sort();
    if sent.text() != Some(expected_text.as_str())
        || sent_entities.len() != expected_custom
        || returned_entities.len() != expected_custom
        || returned_entities != expected_entities
    {
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
    let mut output = ids
        .iter()
        .take(4)
        .map(|id| {
            let mut display = String::new();
            for ch in id.chars().take(24) {
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
            if id.chars().count() > 24 {
                display.push('…');
            }
            display
        })
        .collect::<Vec<_>>()
        .join(", ");
    if ids.len() > 4 {
        output.push_str(&format!(", … ещё {} ID", ids.len() - 4));
    }
    output
}

#[cfg(feature = "emoji")]
fn first_seen_bounded_ids(ids: &[String], limit: usize) -> (Vec<String>, bool) {
    let mut seen = std::collections::HashSet::new();
    let mut unique = Vec::new();
    for id in ids {
        if !seen.insert(id.as_str()) {
            continue;
        }
        if unique.len() == limit {
            return (unique, true);
        }
        unique.push(id.clone());
    }
    (unique, false)
}

#[cfg(feature = "emoji")]
struct EmojiMessagePlan {
    text: String,
    entities: Vec<MessageEntity>,
    unresolved: Vec<String>,
    truncated: bool,
}

#[cfg(feature = "emoji")]
fn build_custom_emoji_message(
    ids: &[String],
    metadata: &HashMap<String, String>,
) -> EmojiMessagePlan {
    let mut plan = EmojiMessagePlan {
        text: String::new(),
        entities: Vec::new(),
        unresolved: Vec::new(),
        truncated: false,
    };
    let mut text_utf16 = 0usize;
    let mut seen_unresolved = std::collections::HashSet::new();
    for id in ids {
        let Some(label) = metadata
            .get(id)
            .filter(|label| valid_canonical_emoji(label))
        else {
            if seen_unresolved.insert(id.as_str()) {
                plan.unresolved.push(id.clone());
            }
            continue;
        };
        if plan.entities.len() >= MAX_OUTPUT_ENTITIES {
            plan.truncated = true;
            break;
        }
        let separator = usize::from(!plan.text.is_empty());
        let offset = text_utf16.saturating_add(separator);
        let length = label.encode_utf16().count();
        if offset.saturating_add(length) > MAX_OUTPUT_UTF16 {
            plan.truncated = true;
            break;
        }
        if separator != 0 {
            plan.text.push(' ');
            text_utf16 = text_utf16.saturating_add(1);
        }
        plan.text.push_str(label);
        text_utf16 = text_utf16.saturating_add(length);
        plan.entities.push(MessageEntity::custom_emoji(
            CustomEmojiId(id.clone()),
            offset,
            length,
        ));
    }
    plan
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "emoji")]
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    #[cfg(feature = "emoji")]
    use axum::{Json, Router, extract::State, routing::post};
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

    #[test]
    fn emoji_ids_report_is_bounded_and_counts_omitted_occurrences() {
        let ids = (0..500)
            .map(|index| format!("same-{}-{}", index % 3, "x".repeat(120)))
            .collect::<Vec<_>>();
        let report = super::format_custom_emoji_ids(&ids);
        assert!(report.encode_utf16().count() <= super::MAX_CUSTOM_EMOJI_IDS_OUTPUT_UTF16);
        assert!(report.contains("ID не показано"));
        assert!(report.contains("<code>same-0-"));
    }

    #[test]
    fn emoji_ids_exact_output_boundary_reserves_omitted_count_and_marks_truncated_ids() {
        let ids = vec!["&".repeat(49); 14];
        let report = super::format_custom_emoji_ids(&ids);
        assert!(report.encode_utf16().count() <= super::MAX_CUSTOM_EMOJI_IDS_OUTPUT_UTF16);
        assert!(report.contains("… и ещё 1 ID не показано"));
        assert!(report.contains(&format!("{}…</code>", "&amp;".repeat(48))));
    }

    #[cfg(feature = "emoji")]
    #[test]
    fn resolver_admission_keeps_first_seen_ids_and_reports_overflow() {
        let ids = ["z", "a", "z", "b"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(
            super::first_seen_bounded_ids(&ids, 2),
            (vec!["z".to_owned(), "a".to_owned()], true)
        );
        assert_eq!(
            super::first_seen_bounded_ids(&ids, 3),
            (vec!["z".to_owned(), "a".to_owned(), "b".to_owned()], false)
        );
    }

    #[cfg(feature = "emoji")]
    #[test]
    fn output_entities_use_utf16_offsets_and_preserve_repeated_ids() {
        let ids = vec!["one".into(), "one".into(), "two".into()];
        let metadata = [("one".into(), "🤦🏽‍♀️".into()), ("two".into(), "😎".into())]
            .into_iter()
            .collect();
        let plan = super::build_custom_emoji_message(&ids, &metadata);
        assert_eq!(plan.text, "🤦🏽‍♀️ 🤦🏽‍♀️ 😎");
        assert!(plan.unresolved.is_empty());
        assert!(!plan.truncated);
        assert_eq!(plan.entities.len(), 3);
        assert_eq!((plan.entities[0].offset, plan.entities[0].length), (0, 7));
        assert_eq!((plan.entities[1].offset, plan.entities[1].length), (8, 7));
        assert_eq!((plan.entities[2].offset, plan.entities[2].length), (16, 2));
    }

    #[cfg(feature = "emoji")]
    #[test]
    fn invalid_metadata_is_not_sent_as_a_unicode_success() {
        let ids = vec!["broken".into()];
        let metadata = [("broken".into(), "not emoji".into())]
            .into_iter()
            .collect();
        let plan = super::build_custom_emoji_message(&ids, &metadata);
        assert!(plan.text.is_empty());
        assert!(plan.entities.is_empty());
        assert_eq!(plan.unresolved, ["broken"]);
    }

    #[test]
    fn late_negative_cache_write_preserves_positive_but_positive_replaces_negative() {
        let now = std::time::Instant::now();
        let mut cache = std::collections::HashMap::new();
        super::cache_metadata_result(
            &mut cache,
            "race-id",
            super::MetadataLookupState::ConfirmedMissing,
            now,
        );
        super::cache_metadata_result(
            &mut cache,
            "race-id",
            super::MetadataLookupState::Positive("🤖".to_owned()),
            now,
        );
        super::cache_metadata_result(
            &mut cache,
            "race-id",
            super::MetadataLookupState::TransientFailure,
            now,
        );
        assert_eq!(
            cache.get("race-id").and_then(|entry| match &entry.state {
                super::MetadataLookupState::Positive(value) => Some(value.as_str()),
                _ => None,
            }),
            Some("🤖")
        );

        super::cache_metadata_result(
            &mut cache,
            "race-id",
            super::MetadataLookupState::Positive("😎".to_owned()),
            now,
        );
        assert_eq!(
            cache.get("race-id").and_then(|entry| match &entry.state {
                super::MetadataLookupState::Positive(value) => Some(value.as_str()),
                _ => None,
            }),
            Some("😎")
        );
    }

    #[cfg(feature = "emoji")]
    #[tokio::test]
    async fn resolver_batches_reorders_partial_results_and_caches_missing_ids() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock Bot API");
        let address = listener.local_addr().expect("mock Bot API address");
        let request_count = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .fallback(post(serve_emoji_lookup))
            .with_state(Arc::clone(&request_count));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let bot = teloxide::Bot::new("fixture")
            .set_api_url(format!("http://{address}/").parse().expect("mock URL"))
            .parse_mode(ParseMode::Html);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let ids = (0..601)
            .map(|index| format!("resolver-{nonce}-{index:03}"))
            .collect::<Vec<_>>();

        let first = super::resolve_custom_emoji_fallbacks(&bot, &ids).await;
        assert_eq!(first.len(), 600, "one confirmed missing ID stays absent");
        assert_eq!(first.get(&ids[1]).map(String::as_str), Some("🤖"));
        assert_eq!(first.get(&ids[2]).map(String::as_str), Some("😎"));
        assert!(!first.contains_key(&ids[0]));
        let still_cached = super::resolve_custom_emoji_fallbacks(&bot, &ids[600..]).await;
        assert_eq!(still_cached.len(), 1);
        assert_eq!(request_count.load(Ordering::SeqCst), 4);
        server.abort();
    }

    #[cfg(feature = "emoji")]
    async fn serve_emoji_lookup(
        State(request_count): State<Arc<AtomicUsize>>,
        Json(payload): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        request_count.fetch_add(1, Ordering::SeqCst);
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
            .filter(|id| !id.ends_with("-000"))
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
        Json(serde_json::json!({"ok":true,"result":stickers}))
    }
}
