use proptest::{collection, prelude::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedIds {
    pub instance: String,
    pub chat_id: i64,
    pub user_id: u64,
    pub message_id: i32,
}

pub fn scoped_ids() -> impl Strategy<Value = ScopedIds> {
    (
        "[a-z][a-z0-9_]{0,11}",
        prop_oneof![1_i64..=9_999_999_999, -100_000_000_000_i64..=-1],
        1_u64..=9_999_999_999,
        1_i32..=2_000_000_000,
    )
        .prop_map(|(instance, chat_id, user_id, message_id)| ScopedIds {
            instance,
            chat_id,
            user_id,
            message_id,
        })
}

fn unicode_token() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("a".to_owned()),
        Just("Ж".to_owned()),
        Just("😀".to_owned()),
        Just("🧑‍💻".to_owned()),
        Just("&".to_owned()),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Utf16EntityCase {
    pub text: String,
    pub kind: &'static str,
    pub offset: u32,
    pub length: u32,
}

pub fn utf16_entity_case() -> impl Strategy<Value = Utf16EntityCase> {
    collection::vec(unicode_token(), 1..=10).prop_flat_map(|tokens| {
        let token_count = tokens.len();
        (0..token_count).prop_flat_map(move |start| {
            let span_max = token_count - start;
            (1..=span_max).prop_map({
                let tokens = tokens.clone();
                move |span| {
                    let offset = tokens[..start]
                        .iter()
                        .map(|token| token.encode_utf16().count() as u32)
                        .sum();
                    let length = tokens[start..start + span]
                        .iter()
                        .map(|token| token.encode_utf16().count() as u32)
                        .sum();
                    Utf16EntityCase {
                        text: tokens.concat(),
                        kind: "bold",
                        offset,
                        length,
                    }
                }
            })
        })
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaValidUpdateCase {
    pub ids: ScopedIds,
    pub text: Utf16EntityCase,
    pub update_id: u32,
}

impl SchemaValidUpdateCase {
    pub fn to_json(&self) -> Value {
        json!({
            "update_id": self.update_id,
            "message": {
                "message_id": self.ids.message_id,
                "date": 1_700_000_000_i64,
                "chat": {
                    "id": if self.ids.chat_id < 0 { self.ids.chat_id } else { -self.ids.chat_id },
                    "type": "supergroup",
                    "title": "synthetic test group"
                },
                "from": {
                    "id": self.ids.user_id,
                    "is_bot": false,
                    "first_name": "Synthetic"
                },
                "text": self.text.text,
                "entities": [{
                    "type": self.text.kind,
                    "offset": self.text.offset,
                    "length": self.text.length
                }]
            }
        })
    }
}

pub fn schema_valid_update() -> impl Strategy<Value = SchemaValidUpdateCase> {
    (scoped_ids(), utf16_entity_case(), 1_u32..=u32::MAX).prop_map(|(ids, text, update_id)| {
        SchemaValidUpdateCase {
            ids,
            text,
            update_id,
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorRole {
    Owner,
    Admin,
    Member,
    Anonymous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Read,
    Moderate,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationCase {
    pub actor: ActorRole,
    pub capability: Capability,
}

pub fn authorization_case() -> impl Strategy<Value = AuthorizationCase> {
    (
        prop_oneof![
            Just(ActorRole::Owner),
            Just(ActorRole::Admin),
            Just(ActorRole::Member),
            Just(ActorRole::Anonymous),
        ],
        prop_oneof![
            Just(Capability::Read),
            Just(Capability::Moderate),
            Just(Capability::Write),
        ],
    )
        .prop_map(|(actor, capability)| AuthorizationCase { actor, capability })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HttpOutcome {
    Response { status: u16, body_bytes: usize },
    Timeout,
    DisconnectedBeforeResponse,
    DisconnectedAfterAcceptance,
    Cancelled,
}

pub fn http_outcome() -> impl Strategy<Value = HttpOutcome> {
    prop_oneof![
        (100_u16..=599, 0_usize..=4096)
            .prop_map(|(status, body_bytes)| { HttpOutcome::Response { status, body_bytes } }),
        Just(HttpOutcome::Timeout),
        Just(HttpOutcome::DisconnectedBeforeResponse),
        Just(HttpOutcome::DisconnectedAfterAcceptance),
        Just(HttpOutcome::Cancelled),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum JobPhase {
    Pending { generation: u32 },
    Claimed { generation: u32 },
    Completed { generation: u32 },
    Retryable { generation: u32, attempts: u16 },
    Failed { generation: u32, attempts: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum JobOperation {
    Claim,
    Complete { generation: u32 },
    Retry { generation: u32 },
    ExpireLease,
}

pub fn job_phase() -> impl Strategy<Value = JobPhase> {
    (any::<u8>(), 1_u32..=100_000, 0_u16..=100).prop_map(
        |(phase, generation, attempts)| match phase % 5 {
            0 => JobPhase::Pending { generation },
            1 => JobPhase::Claimed { generation },
            2 => JobPhase::Completed { generation },
            3 => JobPhase::Retryable {
                generation,
                attempts,
            },
            _ => JobPhase::Failed {
                generation,
                attempts,
            },
        },
    )
}

pub fn ordered_job_operations() -> impl Strategy<Value = Vec<JobOperation>> {
    collection::vec(
        prop_oneof![
            Just(JobOperation::Claim),
            (1_u32..=100_000).prop_map(|generation| JobOperation::Complete { generation }),
            (1_u32..=100_000).prop_map(|generation| JobOperation::Retry { generation }),
            Just(JobOperation::ExpireLease),
        ],
        0..=32,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    Text,
    Entity,
    Caption,
    Qr,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub target: u8,
    pub source: EvidenceSource,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct IngressMessageKey {
    pub instance: String,
    pub chat_id: i64,
    pub user_id: u64,
    pub message_id: i32,
}

pub fn ingress_message_key() -> impl Strategy<Value = IngressMessageKey> {
    (
        "[a-z][a-z0-9_]{0,11}",
        -100_000_000_000_i64..=-1,
        1_u64..=9_999_999_999,
        1_i32..=2_000_000_000,
    )
        .prop_map(
            |(instance, chat_id, user_id, message_id)| IngressMessageKey {
                instance,
                chat_id,
                user_id,
                message_id,
            },
        )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum IngressOperation {
    Receive {
        revision: u16,
        media_generation: u8,
        evidence: Vec<EvidenceRef>,
    },
    Duplicate {
        revision: u16,
    },
    Edit {
        revision: u16,
        media_generation: u8,
        evidence: Vec<EvidenceRef>,
    },
    RemoveLink {
        revision: u16,
    },
    AttachQr {
        media_generation: u8,
        targets: Vec<u8>,
    },
    ReplaceMedia {
        revision: u16,
        media_generation: u8,
    },
    DeleteStoredMessage,
    RefreshShadow {
        revision: u16,
    },
    RetryOldJob {
        media_generation: u8,
        targets: Vec<u8>,
    },
}

fn evidence_refs() -> impl Strategy<Value = Vec<EvidenceRef>> {
    collection::vec(
        (
            0_u8..=15,
            prop_oneof![
                Just(EvidenceSource::Text),
                Just(EvidenceSource::Entity),
                Just(EvidenceSource::Caption),
            ],
        )
            .prop_map(|(target, source)| EvidenceRef { target, source }),
        0..=8,
    )
}

fn target_refs() -> impl Strategy<Value = Vec<u8>> {
    collection::vec(0_u8..=15, 0..=8)
}

pub fn ingress_operation() -> impl Strategy<Value = IngressOperation> {
    prop_oneof![
        2 => (1_u16..=100, 0_u8..=15, evidence_refs()).prop_map(|(revision, media_generation, evidence)| IngressOperation::Receive { revision, media_generation, evidence }),
        2 => (1_u16..=100).prop_map(|revision| IngressOperation::Duplicate { revision }),
        3 => (1_u16..=100, 0_u8..=15, evidence_refs()).prop_map(|(revision, media_generation, evidence)| IngressOperation::Edit { revision, media_generation, evidence }),
        1 => (1_u16..=100).prop_map(|revision| IngressOperation::RemoveLink { revision }),
        2 => (0_u8..=15, target_refs()).prop_map(|(media_generation, targets)| IngressOperation::AttachQr { media_generation, targets }),
        2 => (1_u16..=100, 0_u8..=15).prop_map(|(revision, media_generation)| IngressOperation::ReplaceMedia { revision, media_generation }),
        1 => Just(IngressOperation::DeleteStoredMessage),
        2 => (1_u16..=100).prop_map(|revision| IngressOperation::RefreshShadow { revision }),
        1 => (0_u8..=15, target_refs()).prop_map(|(media_generation, targets)| IngressOperation::RetryOldJob { media_generation, targets }),
    ]
}

pub fn ingress_trace() -> impl Strategy<Value = Vec<IngressOperation>> {
    collection::vec(ingress_operation(), 1..=32)
}
