//! Дешёвое review-only решение до LLM. Корпус обновляется ручными метками в БД.
use super::repo::NewUserAuditJob;
use crate::config::Config;
pub use crate::config_file::{LocalCascadeConfig, LocalCascadeMode};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use std::collections::{BTreeSet, HashSet};
use teloxide_antispam::{
    logreg::{LinearSpamModel, spam_probability},
    preprocess::prepare_text,
    text::{jaccard, token_set},
};

pub const VERSION: &str = "local-audit-cascade-v1";
pub const METADATA_KEY: &str = "_local_cascade";
pub const REVIEW_LABEL: &str = "local_cascade_review_candidate";
const CORPUS_MESSAGES_PER_AUTHOR: i64 = 10;

// Последнее решение, а не старый spam stamp, разрешает каждую строку корпуса.
// System labels не являются ручным подтверждением правильности классификатора.
const CORPUS_SQL: &str = r#"
with latest as (
 select distinct on (chat_id, telegram_user_id) id, chat_id, telegram_user_id, label, source, created_at
 from spam_label_events where chat_id=$1
 order by chat_id, telegram_user_id, created_at desc, id desc
), examples as (
 select l.chat_id,l.telegram_user_id as user_id,l.id as label_event_id,l.label,
        l.created_at as label_at,m.message_id,left(m.text,6000) as text,m.created_at as message_at,
        row_number() over(partition by l.telegram_user_id order by m.created_at,m.message_id) as ordinal
 from latest l join telegram_messages m on m.chat_id=l.chat_id and m.user_id=l.telegram_user_id
 where l.source in ('owner_manual','owner_review','legacy_backfill') and l.label in ('spam','not_spam')
   and m.source_channel_id is null and m.created_at<=l.created_at
   and (m.edited_at is null or m.edited_at<=l.created_at)
   and nullif(trim(m.text),'') is not null
   and (l.label='not_spam' or (m.spam_marked_at is not null and m.spam_source='manual_owner_confirmation'))
)
select chat_id,user_id,label_event_id,label,label_at,message_id,text,message_at
from examples where ordinal <= $2 order by label_event_id,message_id limit $3
"#;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(deny_unknown_fields)]
pub struct CorpusExample {
    pub chat_id: i64,
    pub user_id: i64,
    pub label_event_id: i64,
    pub label: String,
    pub label_at: DateTime<Utc>,
    pub message_id: i32,
    pub text: String,
    pub message_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalEvidence {
    pub version: String,
    pub applied: bool,
    pub candidate: bool,
    pub reason: String,
    pub text_hash: String,
    pub policy_hash: String,
    pub model_version: Option<String>,
    pub linear_probability: Option<f64>,
    pub template_authors: usize,
    pub template_sources: Vec<(i64, i64, i32)>,
    pub normal_similarity: f64,
    pub message_slot: Option<usize>,
    pub corpus_hash: String,
}
/// Тот же bounded/redacted input, что находится в canonical audit snapshot.
/// Replay и offline training не должны видеть больше текста, чем live worker.
pub fn model_input(text: &str) -> String {
    crate::features::new_user_analysis::bounded_audit_text(Some(text)).unwrap_or_default()
}
pub fn hash_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub async fn load_corpus(
    pool: &PgPool,
    chat_id: i64,
    max_examples: usize,
) -> anyhow::Result<Vec<CorpusExample>> {
    Ok(sqlx::query_as(CORPUS_SQL)
        .bind(chat_id)
        .bind(CORPUS_MESSAGES_PER_AUTHOR)
        .bind(i64::try_from(max_examples.saturating_add(1))?)
        .fetch_all(pool)
        .await?)
}

/// Нормализация корпуса выполняется один раз на job/replay, а не для каждого текста.
pub struct PreparedCorpus {
    examples: Vec<(CorpusExample, BTreeSet<String>)>,
    hash: String,
}
impl PreparedCorpus {
    pub fn new(corpus: &[CorpusExample]) -> anyhow::Result<Self> {
        anyhow::ensure!(
            corpus
                .iter()
                .all(|e| matches!(e.label.as_str(), "spam" | "not_spam")),
            "unsupported corpus label"
        );
        Ok(Self {
            hash: hash_bytes(&serde_json::to_vec(corpus)?),
            examples: corpus
                .iter()
                .map(|e| {
                    (
                        e.clone(),
                        token_set(&prepare_text(&model_input(&e.text)).model_text()),
                    )
                })
                .collect(),
        })
    }
    /// Offline replay ограничивает источники временем решения.
    /// Сам кандидат не может подтверждать шаблон спама.
    pub fn evaluate(
        &self,
        text: &str,
        chat_id: i64,
        user_id: i64,
        policy: &LocalCascadeConfig,
        model: Option<&LinearSpamModel>,
        as_of: Option<DateTime<Utc>>,
    ) -> anyhow::Result<LocalEvidence> {
        policy.validate()?;
        let input = model_input(text);
        let normalized = prepare_text(&input).model_text();
        let tokens = token_set(&normalized);
        let probability = model
            .map(|m| spam_probability(m, &input))
            .filter(|p| p.is_finite() && (0.0..=1.0).contains(p));
        let mut evidence = LocalEvidence {
            version: VERSION.into(),
            applied: false,
            candidate: false,
            reason: "insufficient_evidence".into(),
            text_hash: hash_bytes(text.as_bytes()),
            policy_hash: hash_bytes(&serde_json::to_vec(policy)?),
            model_version: model.map(|m| m.version.clone()),
            linear_probability: probability,
            template_authors: 0,
            template_sources: Vec::new(),
            normal_similarity: 0.0,
            message_slot: None,
            corpus_hash: self.hash.clone(),
        };
        if self.examples.len() > policy.max_corpus_examples {
            evidence.reason = "corpus_limit".into();
            return Ok(evidence);
        }
        if normalized.chars().count() < policy.min_text_chars || tokens.len() < policy.min_tokens {
            evidence.reason = "short_text".into();
            return Ok(evidence);
        }
        let mut authors = HashSet::new();
        for (example, source_tokens) in self.examples.iter().filter(|(e, _)| {
            e.chat_id == chat_id
                && as_of.is_none_or(|time| e.label_at < time && e.message_at < time)
        }) {
            if example.label == "not_spam" && example.user_id == user_id {
                evidence.reason = "manual_not_spam".into();
                return Ok(evidence);
            }
            if example.user_id == user_id {
                continue;
            }
            let similarity = jaccard(&tokens, source_tokens);
            if example.label == "spam"
                && similarity >= policy.template_similarity
                && authors.insert(example.user_id)
                && evidence.template_sources.len() < 20
            {
                evidence.template_sources.push((
                    example.label_event_id,
                    example.user_id,
                    example.message_id,
                ));
            } else if example.label == "not_spam" {
                evidence.normal_similarity = evidence.normal_similarity.max(similarity);
            }
        }
        evidence.template_authors = authors.len();
        if evidence.normal_similarity >= policy.normal_veto_similarity {
            evidence.reason = "confirmed_normal_neighbor".into();
            return Ok(evidence);
        }
        evidence.reason = if authors.len() >= policy.min_template_authors {
            "confirmed_templates"
        } else if probability.is_some_and(|p| p >= policy.linear_review_threshold) {
            "linear_model"
        } else {
            "insufficient_evidence"
        }
        .into();
        evidence.candidate = matches!(
            evidence.reason.as_str(),
            "confirmed_templates" | "linear_model"
        );
        evidence.applied = evidence.candidate && policy.mode == LocalCascadeMode::ReviewOnly;
        Ok(evidence)
    }
}
pub fn evaluate(
    text: &str,
    chat_id: i64,
    user_id: i64,
    corpus: &[CorpusExample],
    policy: &LocalCascadeConfig,
    model: Option<&LinearSpamModel>,
    as_of: Option<DateTime<Utc>>,
) -> anyhow::Result<LocalEvidence> {
    PreparedCorpus::new(corpus)?.evaluate(text, chat_id, user_id, policy, model, as_of)
}
pub fn metadata(value: &Value) -> anyhow::Result<Option<LocalEvidence>> {
    let Some(value) = value.get(METADATA_KEY) else {
        return Ok(None);
    };
    let evidence: LocalEvidence = serde_json::from_value(value.clone())?;
    anyhow::ensure!(
        evidence.version == VERSION,
        "unsupported local cascade version"
    );
    anyhow::ensure!(
        !evidence.applied || evidence.candidate,
        "invalid applied local verdict"
    );
    Ok(Some(evidence))
}
/// Невалидный локальный assessment может только убрать привилегии.
pub fn blocks_enforcement(value: Option<&Value>) -> bool {
    match value.map(metadata) {
        Some(Ok(Some(e))) => e.applied,
        Some(Err(_)) => true,
        _ => false,
    }
}
pub fn review_assessment(evidence: &LocalEvidence) -> Value {
    json!({
        "avatar_observation": null,
        "first_message_assessment": {
            "relation_to_chat":"loosely_related", "direct_dm_offer":false,"offtopic_promo":false,
            "template_campaign":false,"self_reference_grammar":"none_or_unclear",
            "profile_name_grammar_relation":"not_applicable","risk_markers":[],"evidence":[],
            "summary":"Локальные текстовые признаки требуют ручной проверки; смысл и контекст не подтверждены.","confidence":0.0
        },
        "profile_assessment": {"risk_patterns":[],"evidence":[],"contradictions":[],
            "review_priority":"medium","confidence":0.0,"summary":"Профиль и аватар этим локальным решением не оценивались."},
        (METADATA_KEY): evidence,
    })
}
pub async fn evaluate_job(
    pool: &PgPool,
    config: &Config,
    job: &NewUserAuditJob,
) -> anyhow::Result<Option<LocalEvidence>> {
    let policy = &config.community.moderation.local_cascade;
    if job.chat_id >= 0
        || policy.mode == LocalCascadeMode::Disabled
        || !config.community.moderation.enabled
        || !crate::features::ingest::managed_chat_allows(config, job.chat_id, |c| c.moderation)
    {
        return Ok(None);
    }
    let Some(text) = job.input_json["text"]["first_message_preview"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
    else {
        return Ok(None);
    };
    let count = job.input_json["activity"]["message_count"].as_i64();
    let age = job.input_json["activity"]["chat_age_sec"].as_i64();
    if !count.is_some_and(|n| (1..=policy.max_messages).contains(&n))
        || !age.is_some_and(|n| (0..=policy.max_chat_age_seconds).contains(&n))
    {
        return Ok(None);
    }
    let state: Option<(bool, i32, i32, Value, bool)> = sqlx::query_as(
        "select coalesce(u.is_spammer,false),a.risk_avatar_score,a.risk_personal_channel_score,a.risk_signal_breakdown, coalesce((select e.label from spam_label_events e where e.chat_id=a.chat_id and e.telegram_user_id=a.telegram_user_id order by e.created_at desc,e.id desc limit 1)='not_spam',false) from telegram_new_user_profile_audits a left join telegram_chat_users u on u.chat_id=a.chat_id and u.telegram_user_id=a.telegram_user_id where a.chat_id=$1 and a.telegram_user_id=$2"
    ).bind(job.chat_id).bind(job.telegram_user_id).fetch_optional(pool).await?;
    let Some((known, avatar, channel, signals, manual_normal)) = state else {
        return Ok(None);
    };
    if known
        || manual_normal
        || avatar > 0
        || channel > 0
        || !crate::features::auto_moderation::qualifying_auto_ban_evidence(&signals).is_empty()
    {
        return Ok(None);
    }
    let corpus = load_corpus(pool, job.chat_id, policy.max_corpus_examples).await?;
    let corpus = PreparedCorpus::new(&corpus)?;
    let mut texts = vec![text];
    if let Some(recent) = job.input_json["text"]["recent_message_previews"].as_array() {
        texts.extend(recent.iter().filter_map(Value::as_str).take(10));
    }
    let mut seen = HashSet::new();
    let mut fallback = None;
    for (slot, text) in texts
        .into_iter()
        .enumerate()
        .filter(|(_, t)| seen.insert(*t))
    {
        let mut evidence = corpus.evaluate(
            text,
            job.chat_id,
            job.telegram_user_id,
            policy,
            config.linear_spam_model.as_deref(),
            None,
        )?;
        evidence.message_slot = Some(slot);
        if evidence.candidate {
            return Ok(Some(evidence));
        }
        fallback.get_or_insert(evidence);
    }
    Ok(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    const CHAT: i64 = -101;
    const TEXT: &str = "Нужны срочно деньги сегодня напишите мне в личные сообщения";
    fn policy() -> LocalCascadeConfig {
        LocalCascadeConfig {
            mode: LocalCascadeMode::ReviewOnly,
            ..Default::default()
        }
    }
    fn example(user: i64, label: &str) -> CorpusExample {
        CorpusExample {
            chat_id: CHAT,
            user_id: user,
            label_event_id: user,
            label: label.into(),
            label_at: Utc::now() - chrono::Duration::hours(1),
            message_id: user as i32,
            text: TEXT.into(),
            message_at: Utc::now() - chrono::Duration::hours(2),
        }
    }
    #[test]
    fn unicode_variants_use_reviewed_templates_without_new_phrase_rules() {
        let corpus = [example(1, "spam"), example(2, "spam")];
        let mut variants = vec![
            TEXT.to_uppercase(),
            format!("!!! {TEXT} ???"),
            TEXT.replace("мне", "мнe"),
            TEXT.replace("срочно", "сpочно"),
            TEXT.replace("сообщения", "сοобщения"),
        ];
        for c in [
            '\u{200b}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{feff}', '\u{00ad}', '\u{202e}',
            '\u{2066}', '\u{034f}', '\u{fe0f}',
        ] {
            variants.push(TEXT.replace("деньги", &format!("де{c}ньги")));
        }
        for space in [
            '\u{00a0}', '\u{202f}', '\u{2009}', '\u{2003}', '\t', '\n', '\r', '\u{3000}',
            '\u{2007}', '\u{205f}',
        ] {
            variants.push(TEXT.replace(' ', &space.to_string()));
        }
        assert_eq!(variants.len(), 25);
        for text in variants {
            let e = evaluate(&text, CHAT, 99, &corpus, &policy(), None, None).unwrap();
            assert!(
                e.applied,
                "variant hash={} reason={}",
                e.text_hash, e.reason
            );
            assert_eq!(e.template_authors, 2);
            assert_eq!(e.template_sources.len(), 2);
        }
    }
    #[test]
    fn offline_input_matches_live_bounded_and_redacted_snapshot() {
        let raw = format!(
            "{} https://example.invalid/secret",
            "Длинный текст ".repeat(40)
        );
        let preview = model_input(&raw);
        assert!(preview.chars().count() <= 281);
        assert_eq!(preview, model_input(&preview));
        assert_eq!(
            model_input("Привет  https://example.invalid/private\n всем"),
            "Привет [link] всем"
        );
    }
    #[test]
    fn independent_sources_scope_and_time_are_required() {
        let one = example(1, "spam");
        assert!(
            !evaluate(
                TEXT,
                CHAT,
                99,
                &[one.clone(), one.clone()],
                &policy(),
                None,
                None
            )
            .unwrap()
            .candidate
        );
        assert!(
            !evaluate(
                TEXT,
                CHAT,
                1,
                &[one.clone(), example(2, "spam")],
                &policy(),
                None,
                None
            )
            .unwrap()
            .candidate
        );
        assert!(
            !evaluate(
                TEXT,
                CHAT - 1,
                99,
                &[one.clone(), example(2, "spam")],
                &policy(),
                None,
                None
            )
            .unwrap()
            .candidate
        );
        assert!(
            !evaluate(
                TEXT,
                CHAT,
                99,
                &[one, example(2, "spam")],
                &policy(),
                None,
                Some(Utc::now() - chrono::Duration::days(1))
            )
            .unwrap()
            .candidate
        );
    }
    #[test]
    fn normal_neighbors_and_own_corrections_veto_templates() {
        let mut rows = vec![
            example(1, "spam"),
            example(2, "spam"),
            example(3, "not_spam"),
        ];
        assert_eq!(
            evaluate(TEXT, CHAT, 99, &rows, &policy(), None, None)
                .unwrap()
                .reason,
            "confirmed_normal_neighbor"
        );
        rows[2].user_id = 99;
        rows[2].text = "Иной обычный текст".into();
        assert_eq!(
            evaluate(TEXT, CHAT, 99, &rows, &policy(), None, None)
                .unwrap()
                .reason,
            "manual_not_spam"
        );
    }
    #[test]
    fn disabled_shadow_short_and_overflow_never_apply() {
        let rows = [example(1, "spam"), example(2, "spam")];
        for mode in [LocalCascadeMode::Disabled, LocalCascadeMode::Shadow] {
            let p = LocalCascadeConfig { mode, ..policy() };
            assert!(
                !evaluate(TEXT, CHAT, 99, &rows, &p, None, None)
                    .unwrap()
                    .applied
            );
        }
        assert_eq!(
            evaluate("деньги", CHAT, 99, &rows, &policy(), None, None)
                .unwrap()
                .reason,
            "short_text"
        );
        let p = LocalCascadeConfig {
            max_corpus_examples: 10,
            ..policy()
        };
        let big = (1..=11).map(|u| example(u, "spam")).collect::<Vec<_>>();
        assert_eq!(
            evaluate(TEXT, CHAT, 99, &big, &p, None, None)
                .unwrap()
                .reason,
            "corpus_limit"
        );
        let invalid = LocalCascadeConfig {
            template_similarity: f64::NAN,
            ..policy()
        };
        assert!(evaluate(TEXT, CHAT, 99, &rows, &invalid, None, None).is_err());
    }
    #[test]
    fn linear_model_can_only_route_review_and_normal_veto_still_wins() {
        let model=teloxide_antispam::logreg::load_model(r#"{"analyzer":"word_12_lower","version":"fixture","intercept":7,"coef":[0],"vocab":{"деньги":1}}"#).unwrap();
        let verdict = evaluate(TEXT, CHAT, 99, &[], &policy(), Some(&model), None).unwrap();
        assert_eq!(verdict.reason, "linear_model");
        assert!(verdict.applied);
        assert!(blocks_enforcement(Some(&review_assessment(&verdict))));
        let normal = example(3, "not_spam");
        assert!(
            !evaluate(TEXT, CHAT, 99, &[normal], &policy(), Some(&model), None)
                .unwrap()
                .applied
        );
        let conservative = LocalCascadeConfig {
            linear_review_threshold: 0.999_999,
            ..policy()
        };
        assert!(
            !evaluate(TEXT, CHAT, 99, &[], &conservative, Some(&model), None)
                .unwrap()
                .candidate
        );
    }
    #[test]
    fn local_metadata_is_strict_and_cannot_authorize_enforcement() {
        let e = evaluate(
            TEXT,
            CHAT,
            99,
            &[example(1, "spam"), example(2, "spam")],
            &policy(),
            None,
            None,
        )
        .unwrap();
        let mut assessment = review_assessment(&e);
        assert!(blocks_enforcement(Some(&assessment)));
        assert_eq!(metadata(&assessment).unwrap().unwrap(), e);
        assessment[METADATA_KEY]["candidate"] = json!(false);
        assert!(metadata(&assessment).is_err());
        assert!(blocks_enforcement(Some(&assessment)));
        assert!(!blocks_enforcement(Some(&json!({}))));
        assert!(PreparedCorpus::new(&[example(1, "unreviewed")]).is_err());
    }
}
