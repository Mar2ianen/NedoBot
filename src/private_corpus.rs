//! Offline processing for explicitly authorized, private historical URL exports.
//! This module never performs network requests and never emits source message text.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader, Read},
    ops::Range,
};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use teloxide_url::{Kind, find_urls, parse_telegram_link, parse_url, utf16_range};

const MAX_INPUT_LINE_BYTES: usize = 1_048_576;
type RichTargetSpan = (Range<usize>, String);

#[derive(Debug, Deserialize)]
pub struct RawMessage {
    pub message_id: i64,
    pub created_at: String,
    pub text: Option<Value>,
    pub caption: Option<Value>,
    #[serde(default)]
    pub entities: Value,
    #[serde(default)]
    pub caption_entities: Value,
    #[serde(default)]
    pub rich_message: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct RawQrEvidence {
    pub message_id: i64,
    pub created_at: String,
    pub target_sha256: String,
    pub host: Option<String>,
    pub display_hint: String,
    pub privacy_sensitive: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PrivateUrlRecord {
    pub schema_version: u8,
    pub instance: String,
    pub validity_class: &'static str,
    pub evidence_source: &'static str,
    pub bot_api_version: &'static str,
    pub teloxide_revision: &'static str,
    pub cross_field_assumptions: &'static str,
    pub message_id: i64,
    pub observed_at: String,
    pub canonical_url_identity: Option<String>,
    pub class: String,
    pub source: String,
    pub label: Option<String>,
    pub label_provenance: String,
    pub qr_target_sha256: Option<String>,
    pub qr_host: Option<String>,
    pub qr_display_hint: Option<String>,
    pub qr_privacy_sensitive: Option<bool>,
    pub qr_url_recoverable: Option<bool>,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct InstanceSummary {
    pub message_rows: usize,
    pub url_records: usize,
    pub unique_url_identities: usize,
    pub qr_hash_only_records: usize,
    pub source_counts: BTreeMap<String, usize>,
    pub class_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct PrivateCorpusSummary {
    pub schema_version: u8,
    pub validity_class: &'static str,
    pub instances: BTreeMap<String, InstanceSummary>,
    pub shared_url_identity_count: usize,
    pub qr_hashes_are_reversible: bool,
    pub labels_are_message_level: bool,
}

/// Process one local, private JSONL message export. Input lines should contain
/// only message ID/date/text/caption/entities; no user identity fields.
pub fn extract_private_records(
    instance: &str,
    messages: impl Read,
    qr_evidence: impl Read,
) -> anyhow::Result<(Vec<PrivateUrlRecord>, InstanceSummary)> {
    validate_instance(instance)?;
    let mut records = Vec::new();
    let mut message_count = 0;
    for (line_index, line) in bounded_lines(messages).enumerate() {
        let line = line.with_context(|| format!("read private message line {}", line_index + 1))?;
        let message: RawMessage = serde_json::from_str(&line)
            .with_context(|| format!("parse private message line {}", line_index + 1))?;
        message_count += 1;
        records.extend(message_urls(instance, &message)?);
    }

    for (line_index, line) in bounded_lines(qr_evidence).enumerate() {
        let line = line.with_context(|| format!("read QR line {}", line_index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let qr: RawQrEvidence = serde_json::from_str(&line)
            .with_context(|| format!("parse QR line {}", line_index + 1))?;
        anyhow::ensure!(
            is_sha256_hex(&qr.target_sha256),
            "QR evidence line {} has invalid hash shape",
            line_index + 1
        );
        records.push(PrivateUrlRecord {
            schema_version: 1,
            instance: instance.to_owned(),
            validity_class: "V1_observed",
            evidence_source: "production_postgres_telegram_message_url_evidence",
            bot_api_version: "not_recorded_in_source_row",
            teloxide_revision: "f05b90be40e9838e009281182abfcc3db2319c41",
            cross_field_assumptions: "QR hash remains non_reversible; no URL inferred",
            message_id: qr.message_id,
            observed_at: qr.created_at,
            canonical_url_identity: None,
            class: "qr_hash_only".to_owned(),
            source: "qr".to_owned(),
            label: None,
            label_provenance: "unlabeled_no_message_level_label_joined".to_owned(),
            qr_target_sha256: Some(qr.target_sha256.to_ascii_lowercase()),
            qr_host: qr.host,
            qr_display_hint: Some(qr.display_hint),
            qr_privacy_sensitive: Some(qr.privacy_sensitive),
            qr_url_recoverable: Some(false),
        });
    }

    records.sort_by(|a, b| {
        (
            &a.observed_at,
            a.message_id,
            &a.source,
            &a.canonical_url_identity,
        )
            .cmp(&(
                &b.observed_at,
                b.message_id,
                &b.source,
                &b.canonical_url_identity,
            ))
    });
    let mut summary = InstanceSummary {
        message_rows: message_count,
        ..InstanceSummary::default()
    };
    let mut unique_urls = BTreeSet::new();
    let mut seen = BTreeSet::new();
    records.retain(|record| {
        let key = (
            record.message_id,
            record.source.clone(),
            record.canonical_url_identity.clone(),
            record.qr_target_sha256.clone(),
        );
        seen.insert(key)
    });
    for record in &records {
        *summary
            .source_counts
            .entry(record.source.clone())
            .or_default() += 1;
        *summary
            .class_counts
            .entry(record.class.clone())
            .or_default() += 1;
        if let Some(identity) = &record.canonical_url_identity {
            unique_urls.insert(identity.clone());
        }
    }
    summary.url_records = records
        .iter()
        .filter(|r| r.canonical_url_identity.is_some())
        .count();
    summary.unique_url_identities = unique_urls.len();
    summary.qr_hash_only_records = records
        .iter()
        .filter(|r| r.qr_target_sha256.is_some())
        .count();
    Ok((records, summary))
}

pub fn summarize_instances(
    instances: BTreeMap<String, InstanceSummary>,
    records_by_instance: &BTreeMap<String, Vec<PrivateUrlRecord>>,
) -> anyhow::Result<PrivateCorpusSummary> {
    anyhow::ensure!(
        instances.len() == 2,
        "exactly two separate instances are required"
    );
    anyhow::ensure!(
        instances.contains_key("nedonews") && instances.contains_key("pvo"),
        "expected separate nedonews and pvo instances"
    );
    let left = url_identities(records_by_instance.get("nedonews"));
    let right = url_identities(records_by_instance.get("pvo"));
    let shared_url_identity_count = left.intersection(&right).count();
    Ok(PrivateCorpusSummary {
        schema_version: 1,
        validity_class: "V1_observed",
        instances,
        shared_url_identity_count,
        qr_hashes_are_reversible: false,
        labels_are_message_level: false,
    })
}

fn url_identities(records: Option<&Vec<PrivateUrlRecord>>) -> BTreeSet<String> {
    records
        .into_iter()
        .flatten()
        .filter_map(|record| record.canonical_url_identity.clone())
        .collect()
}

fn message_urls(instance: &str, message: &RawMessage) -> anyhow::Result<Vec<PrivateUrlRecord>> {
    let mut output = Vec::new();
    if let Some(rich_message) = &message.rich_message {
        for target in rich_link_targets(rich_message, 0) {
            if let Some(record) = url_record(instance, message, &target, "rich_message") {
                output.push(record);
            }
        }
        let projection = crate::telegram::export_rich::project(rich_message);
        for found in find_urls(&projection.text) {
            if found.kind == Kind::Url
                && let Some(record) = url_record(instance, message, &found.value, "rich_message")
            {
                output.push(record);
            }
        }
    }
    for (text_key, entities_key) in [("text", "entities"), ("caption", "caption_entities")] {
        let Some(value) = message.field_value(text_key) else {
            continue;
        };
        let Some((text, rich_targets)) = project_formatted_text(value) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        let entities = message.field_entities(entities_key);
        let mut entity_spans = Vec::new();
        for (span, target) in rich_targets {
            if let Some(record) = url_record(instance, message, &target, "telegram_entity") {
                output.push(record);
                entity_spans.push(span);
            }
        }
        if let Some(entities) = entities {
            for entity in entities {
                let Some(kind) = entity.get("type").and_then(Value::as_str) else {
                    continue;
                };
                if !matches!(kind, "url" | "text_link") {
                    continue;
                }
                let (Some(offset), Some(length)) = (
                    entity.get("offset").and_then(Value::as_u64),
                    entity.get("length").and_then(Value::as_u64),
                ) else {
                    continue;
                };
                let Some(span) = utf16_range(&text, offset as usize, length as usize) else {
                    continue;
                };
                let target = if kind == "text_link" {
                    entity.get("url").and_then(Value::as_str)
                } else {
                    text.get(span.clone())
                };
                let Some(target) = target else { continue };
                if let Some(record) = url_record(instance, message, target, "telegram_entity") {
                    output.push(record);
                    entity_spans.push(span);
                }
            }
        }
        for found in find_urls(&text) {
            if found.kind != Kind::Url
                || found.span.as_ref().is_some_and(|span| {
                    entity_spans
                        .iter()
                        .any(|entity| span.start < entity.end && entity.start < span.end)
                })
            {
                continue;
            }
            if let Some(record) = url_record(instance, message, &found.value, "visible_text") {
                output.push(record);
            }
        }
    }
    Ok(output)
}

fn rich_link_targets(value: &Value, depth: usize) -> Vec<String> {
    const MAX_DEPTH: usize = 64;
    if depth >= MAX_DEPTH {
        return Vec::new();
    }
    match value {
        Value::Array(items) => items
            .iter()
            .flat_map(|item| rich_link_targets(item, depth + 1))
            .collect(),
        Value::Object(object) => {
            let mut found = Vec::new();
            if matches!(
                object.get("type").and_then(Value::as_str),
                Some("text_link" | "url" | "link")
            ) && let Some(target) = object
                .get("href")
                .or_else(|| object.get("url"))
                .and_then(Value::as_str)
                .filter(|target| parse_url(target).is_some())
            {
                found.push(target.to_owned());
            }
            for key in ["blocks", "items", "content", "text", "caption", "credit"] {
                if let Some(child) = object.get(key) {
                    found.extend(rich_link_targets(child, depth + 1));
                }
            }
            found
        }
        _ => Vec::new(),
    }
}

impl RawMessage {
    fn field_value(&self, key: &str) -> Option<&Value> {
        match key {
            "text" => self.text.as_ref(),
            "caption" => self.caption.as_ref(),
            _ => None,
        }
    }

    fn field_entities(&self, key: &str) -> Option<&[Value]> {
        match key {
            "entities" => self.entities.as_array(),
            "caption_entities" => self.caption_entities.as_array(),
            _ => None,
        }
        .map(Vec::as_slice)
    }
}

fn project_formatted_text(value: &Value) -> Option<(String, Vec<RichTargetSpan>)> {
    if let Some(text) = value.as_str() {
        return Some((text.to_owned(), Vec::new()));
    }
    let parts = value.as_array()?;
    let mut text = String::new();
    let mut links = Vec::new();
    for part in parts {
        let (segment, kind, explicit_target) = if let Some(segment) = part.as_str() {
            (segment, None, None)
        } else if let Some(object) = part.as_object() {
            let segment = object.get("text").and_then(Value::as_str).unwrap_or("");
            let kind = object.get("type").and_then(Value::as_str);
            let target = object
                .get("href")
                .or_else(|| object.get("url"))
                .and_then(Value::as_str);
            (segment, kind, target)
        } else {
            continue;
        };
        let start = text.len();
        text.push_str(segment);
        let end = text.len();
        if start == end {
            continue;
        }
        let target = match kind {
            Some("text_link") | Some("link") | Some("url") => explicit_target.or(Some(segment)),
            _ => None,
        };
        if let Some(target) = target.filter(|candidate| parse_url(candidate).is_some()) {
            links.push((start..end, target.to_owned()));
        }
    }
    Some((text, links))
}

fn url_record(
    instance: &str,
    message: &RawMessage,
    raw_target: &str,
    source: &str,
) -> Option<PrivateUrlRecord> {
    let canonical = parse_url(raw_target)?.to_string();
    let class = if parse_telegram_link(&canonical).is_some() {
        "telegram_link"
    } else {
        "web_url"
    };
    Some(PrivateUrlRecord {
        schema_version: 1,
        instance: instance.to_owned(),
        validity_class: "V1_observed",
        evidence_source: "production_postgres_telegram_messages_raw_json",
        bot_api_version: "not_recorded_in_source_row",
        teloxide_revision: "f05b90be40e9838e009281182abfcc3db2319c41",
        cross_field_assumptions: "stored message_id/date/text/caption/entities belong to one persisted message",
        message_id: message.message_id,
        observed_at: message.created_at.clone(),
        canonical_url_identity: Some(canonical),
        class: class.to_owned(),
        source: source.to_owned(),
        label: None,
        label_provenance: "unlabeled_no_message_level_label_joined".to_owned(),
        qr_target_sha256: None,
        qr_host: None,
        qr_display_hint: None,
        qr_privacy_sensitive: None,
        qr_url_recoverable: None,
    })
}

fn bounded_lines(input: impl Read) -> impl Iterator<Item = anyhow::Result<String>> {
    BufReader::new(input).split(b'\n').map(|line| {
        let bytes = line?;
        anyhow::ensure!(
            bytes.len() <= MAX_INPUT_LINE_BYTES,
            "private input line exceeds size bound"
        );
        String::from_utf8(bytes).context("private input contains invalid UTF-8")
    })
}

fn validate_instance(instance: &str) -> anyhow::Result<()> {
    if !matches!(instance, "nedonews" | "pvo" | "synthetic_a" | "synthetic_b") {
        bail!("unknown private corpus instance label");
    }
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::{PrivateUrlRecord, extract_private_records, summarize_instances};
    use std::{collections::BTreeMap, io::Cursor};

    fn raw(message_id: i64, text: &str, entities: &str) -> String {
        serde_json::json!({
            "message_id": message_id,
            "created_at": "2026-11-01T00:00:00Z",
            "text": text,
            "caption": null,
            "entities": serde_json::from_str::<serde_json::Value>(entities).unwrap(),
            "caption_entities": []
        })
        .to_string()
    }

    #[test]
    fn extracts_visible_and_hidden_targets_without_retaining_message_text() {
        let line = raw(
            101,
            "😀 read this https://EXAMPLE.org/path and hidden",
            r#"[{"type":"text_link","offset":42,"length":5,"url":"https://example.org/hidden?secret=fixture"}]"#,
        );
        let (records, summary) = extract_private_records(
            "nedonews",
            Cursor::new(format!("{line}\n")),
            Cursor::new(""),
        )
        .unwrap();
        assert_eq!(summary.message_rows, 1);
        assert_eq!(summary.url_records, 2);
        assert!(
            records
                .iter()
                .any(|r| r.canonical_url_identity.as_deref() == Some("https://example.org/path"))
        );
        assert!(records.iter().any(|r| r.canonical_url_identity.as_deref()
            == Some("https://example.org/hidden?secret=fixture")));
        let output = serde_json::to_string(&records).unwrap();
        assert!(!output.contains("read this"));
        assert!(!output.contains("hidden\""));
        assert!(records.iter().all(|r| r.validity_class == "V1_observed"));
    }

    #[test]
    fn qr_hash_is_kept_as_unrecoverable_and_not_fabricated_as_a_url() {
        let qr = serde_json::json!({
            "message_id": 7,
            "created_at": "2026-11-02T00:00:00Z",
            "target_sha256": "a".repeat(64),
            "host": "example.org",
            "display_hint": "example.org/private",
            "privacy_sensitive": true
        });
        let (records, summary) =
            extract_private_records("pvo", Cursor::new(""), Cursor::new(format!("{}\n", qr)))
                .unwrap();
        assert_eq!(summary.qr_hash_only_records, 1);
        assert_eq!(records[0].class, "qr_hash_only");
        assert_eq!(records[0].canonical_url_identity, None);
        assert_eq!(records[0].qr_url_recoverable, Some(false));
    }

    #[test]
    fn two_instance_summary_counts_collisions_without_emitting_targets() {
        let (a, sa) = extract_private_records(
            "nedonews",
            Cursor::new(raw(1, "https://example.org/shared", "[]")),
            Cursor::new(""),
        )
        .unwrap();
        let (b, sb) = extract_private_records(
            "pvo",
            Cursor::new(raw(2, "https://example.org/shared", "[]")),
            Cursor::new(""),
        )
        .unwrap();
        let mut instances = BTreeMap::new();
        instances.insert("nedonews".to_owned(), sa);
        instances.insert("pvo".to_owned(), sb);
        let mut records = BTreeMap::new();
        records.insert("nedonews".to_owned(), a);
        records.insert("pvo".to_owned(), b);
        let summary = summarize_instances(instances, &records).unwrap();
        assert_eq!(summary.shared_url_identity_count, 1);
        assert!(
            !serde_json::to_string(&summary)
                .unwrap()
                .contains("example.org")
        );
    }

    #[test]
    fn malformed_hash_and_unknown_instance_fail_closed() {
        let qr = serde_json::json!({
            "message_id": 1,
            "created_at": "2026-11-02T00:00:00Z",
            "target_sha256": "not-a-hash",
            "host": null,
            "display_hint": "x",
            "privacy_sensitive": false
        });
        assert!(
            extract_private_records("pvo", Cursor::new(""), Cursor::new(format!("{}\n", qr)))
                .is_err()
        );
        assert!(extract_private_records("other", Cursor::new(""), Cursor::new("")).is_err());
    }

    #[test]
    fn extracts_links_from_serialized_formatted_text_segments() {
        let message = serde_json::json!({
            "message_id": 55,
            "created_at": "2026-11-03T00:00:00Z",
            "text": [
                {"type": "bold", "text": "Read "},
                {"type": "link", "text": "https://example.org/visible"},
                {"type": "text_link", "text": "hidden", "href": "https://example.org/hidden"}
            ],
            "caption": null,
            "entities": [],
            "caption_entities": []
        });
        let (records, _) =
            extract_private_records("pvo", Cursor::new(format!("{message}\n")), Cursor::new(""))
                .unwrap();
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r.canonical_url_identity.as_deref() == Some("https://example.org/visible")));
        assert!(
            records
                .iter()
                .any(|r| r.canonical_url_identity.as_deref() == Some("https://example.org/hidden"))
        );
    }

    #[test]
    fn deduplicates_repeated_identical_entity_targets_within_one_message() {
        let message = serde_json::json!({
            "message_id": 57,
            "created_at": "2026-11-03T00:00:00Z",
            "text": [
                {"type":"text_link", "text":"first", "href":"https://example.org/same"},
                {"type":"text_link", "text":"second", "href":"https://example.org/same"}
            ],
            "caption": null,
            "entities": [],
            "caption_entities": []
        });
        let (records, _) =
            extract_private_records("pvo", Cursor::new(format!("{message}\n")), Cursor::new(""))
                .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].canonical_url_identity.as_deref(),
            Some("https://example.org/same")
        );
    }

    #[test]
    fn extracts_targets_from_stored_rich_message_without_network() {
        let message = serde_json::json!({
            "message_id": 56,
            "created_at": "2026-11-03T00:00:00Z",
            "text": null,
            "caption": null,
            "entities": [],
            "caption_entities": [],
            "rich_message": {
                "blocks": [{"type": "paragraph", "text": [
                    {"type": "text", "text": "Open "},
                    {"type": "url", "url": "https://example.org/rich", "text": "resource"}
                ]}]
            }
        });
        let (records, _) = extract_private_records(
            "nedonews",
            Cursor::new(format!("{message}\n")),
            Cursor::new(""),
        )
        .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source, "rich_message");
        assert_eq!(
            records[0].canonical_url_identity.as_deref(),
            Some("https://example.org/rich")
        );
    }

    #[allow(dead_code)]
    fn _schema_anchor(_: PrivateUrlRecord) {}
}
