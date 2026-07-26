use std::collections::BTreeMap;

use serde::Serialize;
use sqlx::{PgPool, Row};

use crate::config::Config;
use crate::features::memory::embedding::{embed_text_batch, pgvector_literal};
use crate::features::search::types::ResearchPlan;

const LEASE_SECONDS: i64 = 10 * 60;
const MAX_RETRY_ATTEMPTS: i32 = 5;
const SHADOW_CANDIDATE_LIMIT: i64 = 12;
const MAX_CANDIDATES_PER_SIGNAL: usize = 4;
const PRECISE_LITERAL_SCORE: f64 = 1.0;
const BROAD_LITERAL_SCORE: f64 = 0.2;

#[derive(Clone, Debug, Serialize)]
pub struct RetrievalCandidate {
    pub message_id: i32,
    pub text: String,
    pub semantic_score: f64,
    pub lexical_score: f64,
    pub exact_score: f64,
    pub freshness_score: f64,
    pub total_score: f64,
}

#[derive(Debug, Serialize)]
pub struct ExpandedChatContext {
    pub anchor_message_id: i32,
    pub kind: &'static str,
    pub messages: Vec<crate::features::ask::chat_search::ChatMessage>,
}

pub async fn expand_shadow_contexts(
    pool: &PgPool,
    chat_id: i64,
    candidates: &[RetrievalCandidate],
) -> anyhow::Result<Vec<ExpandedChatContext>> {
    let mut contexts = Vec::new();
    for candidate in candidates.iter().take(4) {
        let belongs_to_thread = sqlx::query_scalar::<_, bool>(
            "select exists (select 1 from telegram_messages m where m.chat_id = $1 and m.message_id = $2 and (m.reply_to_message_id is not null or exists (select 1 from telegram_messages child where child.chat_id = m.chat_id and child.reply_to_message_id = m.message_id)))",
        )
        .bind(chat_id)
        .bind(candidate.message_id)
        .fetch_one(pool)
        .await?;
        let (kind, messages) = if belongs_to_thread {
            (
                "reply_thread",
                crate::features::ask::chat_search::reply_thread(
                    pool,
                    chat_id,
                    candidate.message_id,
                )
                .await?,
            )
        } else {
            (
                "neighbor_context",
                crate::features::ask::chat_search::message_context(
                    pool,
                    chat_id,
                    candidate.message_id,
                    3,
                    3,
                )
                .await?,
            )
        };
        contexts.push(ExpandedChatContext {
            anchor_message_id: candidate.message_id,
            kind,
            messages,
        });
    }
    Ok(contexts)
}

pub async fn run_shadow_retrieval(
    pool: &PgPool,
    config: &Config,
    chat_id: i64,
    plan: &ResearchPlan,
) -> anyhow::Result<Vec<RetrievalCandidate>> {
    if !config.chat_retrieval_shadow_enabled {
        return Ok(Vec::new());
    }
    let mut candidates = BTreeMap::new();
    let semantic_queries = plan
        .chat_semantic_queries
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    for embedding in embed_text_batch(config, &semantic_queries).await? {
        let embedding = pgvector_literal(&embedding)?;
        merge_candidates(
            &mut candidates,
            load_semantic_candidates(pool, chat_id, &embedding, config).await?,
        );
    }
    for query in &plan.chat_semantic_queries {
        merge_candidates(
            &mut candidates,
            load_lexical_candidates(pool, chat_id, query, config, false).await?,
        );
    }
    for term in &plan.chat_lexical_terms {
        for variant in literal_variants(term) {
            merge_candidates(
                &mut candidates,
                load_lexical_candidates(pool, chat_id, &variant, config, true).await?,
            );
        }
    }
    let mut candidates = candidates.into_values().collect::<Vec<_>>();
    for candidate in &mut candidates {
        candidate.freshness_score = geometric_freshness(
            candidate.freshness_score,
            config.chat_retrieval_half_life_days,
        );
        candidate.total_score = candidate.semantic_score
            + candidate.lexical_score
            + candidate.exact_score
            + candidate.freshness_score;
    }
    candidates.sort_by(|left, right| right.total_score.total_cmp(&left.total_score));
    Ok(select_diverse_candidates(
        candidates,
        SHADOW_CANDIDATE_LIMIT as usize,
    ))
}

fn select_diverse_candidates(
    candidates: Vec<RetrievalCandidate>,
    limit: usize,
) -> Vec<RetrievalCandidate> {
    let mut selected = Vec::new();
    let mut semantic_count = 0;
    let mut lexical_count = 0;
    let mut exact_count = 0;

    for candidate in &candidates {
        let semantic_available =
            candidate.semantic_score > 0.0 && semantic_count < MAX_CANDIDATES_PER_SIGNAL;
        let lexical_available =
            candidate.lexical_score > 0.0 && lexical_count < MAX_CANDIDATES_PER_SIGNAL;
        let exact_available =
            candidate.exact_score > 0.0 && exact_count < MAX_CANDIDATES_PER_SIGNAL;
        if !(semantic_available || lexical_available || exact_available) {
            continue;
        }
        if candidate.semantic_score > 0.0 {
            semantic_count += 1;
        }
        if candidate.lexical_score > 0.0 {
            lexical_count += 1;
        }
        if candidate.exact_score > 0.0 {
            exact_count += 1;
        }
        selected.push(candidate.clone());
    }

    selected.truncate(limit);
    selected
}

fn merge_candidates(
    candidates: &mut BTreeMap<i32, RetrievalCandidate>,
    found: Vec<RetrievalCandidate>,
) {
    for candidate in found {
        candidates
            .entry(candidate.message_id)
            .and_modify(|current| {
                current.semantic_score = current.semantic_score.max(candidate.semantic_score);
                current.lexical_score = current.lexical_score.max(candidate.lexical_score);
                current.exact_score = current.exact_score.max(candidate.exact_score);
            })
            .or_insert(candidate);
    }
}

async fn load_semantic_candidates(
    pool: &PgPool,
    chat_id: i64,
    embedding: &str,
    config: &Config,
) -> anyhow::Result<Vec<RetrievalCandidate>> {
    load_candidates(pool, chat_id, embedding, config, "semantic", 0.0).await
}

async fn load_lexical_candidates(
    pool: &PgPool,
    chat_id: i64,
    query: &str,
    config: &Config,
    exact: bool,
) -> anyhow::Result<Vec<RetrievalCandidate>> {
    let exact_score = exact
        .then(|| literal_match_score(query))
        .unwrap_or_default();
    load_candidates(
        pool,
        chat_id,
        query,
        config,
        if exact { "exact" } else { "lexical" },
        exact_score,
    )
    .await
}

async fn load_candidates(
    pool: &PgPool,
    chat_id: i64,
    query: &str,
    config: &Config,
    kind: &str,
    exact_score: f64,
) -> anyhow::Result<Vec<RetrievalCandidate>> {
    let rows = sqlx::query(r#"
        select m.message_id, m.text,
               (extract(epoch from (now() - m.created_at)) / 86400.0)::double precision as age_days,
               (case when $5 = 'semantic' then 1.0 - (e.embedding <=> $2::vector) else 0.0 end)::double precision as semantic_score,
               (case when $5 = 'lexical' then greatest(ts_rank_cd(to_tsvector('russian', m.text), websearch_to_tsquery('russian', $2)), ts_rank_cd(to_tsvector('simple', m.text), websearch_to_tsquery('simple', $2))) else 0.0 end)::double precision as lexical_score,
               (case when $5 = 'exact' then $6 else 0.0 end)::double precision as exact_score
        from telegram_messages m
        left join telegram_message_embeddings e on e.chat_id = m.chat_id and e.message_id = m.message_id and e.status = 'ready'
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1 and m.created_at >= now() - ($3 * interval '1 day')
          and m.deleted_by_bot_at is null and m.spam_marked_at is null and m.is_automatic_forward = false
          and m.user_id is not null and coalesce(p.is_bot, false) = false
          and (($5 = 'semantic' and e.embedding is not null) or ($5 = 'lexical' and (to_tsvector('russian', m.text) @@ websearch_to_tsquery('russian', $2) or to_tsvector('simple', m.text) @@ websearch_to_tsquery('simple', $2))) or ($5 = 'exact' and m.text ~* $2))
        order by case when $5 = 'semantic' then e.embedding <=> $2::vector end, m.created_at desc
        limit $4
    "#).bind(chat_id).bind(query).bind(config.chat_retrieval_window_days.clamp(1, 90)).bind(SHADOW_CANDIDATE_LIMIT).bind(kind).bind(exact_score).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|row| RetrievalCandidate {
            message_id: row.get("message_id"),
            text: row.get("text"),
            semantic_score: row.get("semantic_score"),
            lexical_score: row.get("lexical_score"),
            exact_score: row.get("exact_score"),
            freshness_score: row.get("age_days"),
            total_score: 0.0,
        })
        .collect())
}

fn literal_match_score(term: &str) -> f64 {
    let words = term.split_whitespace().count();
    let has_digit = term.chars().any(|character| character.is_ascii_digit());
    let has_identifier_punctuation = term.contains(['+', '_', '-']);
    if words >= 2 || has_digit || has_identifier_punctuation {
        PRECISE_LITERAL_SCORE
    } else {
        BROAD_LITERAL_SCORE
    }
}

pub fn geometric_freshness(age_days: f64, half_life_days: f64) -> f64 {
    2_f64.powf(-age_days.max(0.0) / half_life_days.max(0.1))
}

pub fn literal_variants(term: &str) -> Vec<String> {
    let normalized = crate::text::normalize_cyrillic_homoglyphs(term);
    let mut variants = vec![
        term.to_string(),
        transliterate_latin(term),
        normalized.clone(),
    ];
    variants.push(transliterate_latin(&normalized));
    variants
        .into_iter()
        .map(|value| regex_escape(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn regex_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| {
            if "\\.^$|()[]{}*+?".contains(ch) {
                vec!['\\', ch]
            } else {
                vec![ch]
            }
        })
        .collect()
}

fn transliterate_latin(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch.to_ascii_lowercase() {
            'a' => "а".to_string(),
            'b' => "б".to_string(),
            'c' => "к".to_string(),
            'd' => "д".to_string(),
            'e' => "е".to_string(),
            'f' => "ф".to_string(),
            'g' => "г".to_string(),
            'h' => "х".to_string(),
            'i' => "и".to_string(),
            'j' => "дж".to_string(),
            'k' => "к".to_string(),
            'l' => "л".to_string(),
            'm' => "м".to_string(),
            'n' => "н".to_string(),
            'o' => "о".to_string(),
            'p' => "п".to_string(),
            'q' => "к".to_string(),
            'r' => "р".to_string(),
            's' => "с".to_string(),
            't' => "т".to_string(),
            'u' => "у".to_string(),
            'v' => "в".to_string(),
            'w' => "в".to_string(),
            'x' => "кс".to_string(),
            'y' => "й".to_string(),
            'z' => "з".to_string(),
            _ => ch.to_string(),
        })
        .collect()
}

#[derive(Debug)]
struct EmbeddingJob {
    chat_id: i64,
    message_id: i32,
    text: String,
    attempts: i32,
}

pub async fn enqueue_message_embedding(
    pool: &PgPool,
    chat_id: i64,
    message_id: i32,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into telegram_message_embeddings (chat_id, message_id, status)
        select m.chat_id, m.message_id, 'pending'
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1
          and m.message_id = $2
          and nullif(trim(m.text), '') is not null
          and m.user_id is not null
          and coalesce(p.is_bot, false) = false
          and m.is_automatic_forward = false
          and m.deleted_by_bot_at is null
          and m.spam_marked_at is null
        on conflict (chat_id, message_id) do update set
            embedding = null,
            embedding_model = null,
            status = 'pending',
            attempts = 0,
            next_attempt_at = now(),
            processing_started_at = null,
            lease_expires_at = null,
            error_kind = null,
            updated_at = now()
        where telegram_message_embeddings.status <> 'processing'
        "#,
    )
    .bind(chat_id)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[allow(dead_code)] // Used by the standalone resumable backfill binary.
pub async fn enqueue_backfill_batch(
    pool: &PgPool,
    chat_id: i64,
    limit: i64,
) -> anyhow::Result<usize> {
    let result = sqlx::query(
        r#"
        insert into telegram_message_embeddings (chat_id, message_id, status)
        select m.chat_id, m.message_id, 'pending'
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_message_embeddings e
          on e.chat_id = m.chat_id and e.message_id = m.message_id
        where m.chat_id = $1
          and e.chat_id is null
          and nullif(trim(m.text), '') is not null
          and m.user_id is not null
          and coalesce(p.is_bot, false) = false
          and m.is_automatic_forward = false
          and m.deleted_by_bot_at is null
          and m.spam_marked_at is null
        order by m.created_at asc, m.message_id asc
        limit $2
        on conflict do nothing
        "#,
    )
    .bind(chat_id)
    .bind(limit.clamp(1, 1_000))
    .execute(pool)
    .await?;
    Ok(result.rows_affected() as usize)
}

pub async fn process_next_embedding_batch(pool: &PgPool, config: &Config) -> anyhow::Result<bool> {
    let jobs = claim_embedding_jobs(pool, config.chat_retrieval_embedding_batch_size).await?;
    if jobs.is_empty() {
        return Ok(false);
    }

    let texts = jobs.iter().map(|job| job.text.as_str()).collect::<Vec<_>>();
    match embed_text_batch(config, &texts).await {
        Ok(embeddings) => {
            for (job, embedding) in jobs.iter().zip(embeddings) {
                mark_embedding_ready(pool, job, &embedding, &config.rag_embedding_model).await?;
            }
        }
        Err(err) => {
            let error_kind = if err.to_string().contains("429") {
                "http_429"
            } else {
                "embedding_failed"
            };
            for job in &jobs {
                mark_embedding_failed(pool, job, error_kind).await?;
            }
            tracing::warn!(%err, jobs = jobs.len(), "chat retrieval embedding batch failed");
        }
    }
    Ok(true)
}

async fn claim_embedding_jobs(
    pool: &PgPool,
    batch_size: usize,
) -> anyhow::Result<Vec<EmbeddingJob>> {
    let jobs = claim_pending_embedding_jobs(pool, batch_size).await?;
    if !jobs.is_empty() {
        return Ok(jobs);
    }
    claim_expired_embedding_jobs(pool, batch_size).await
}

async fn claim_pending_embedding_jobs(
    pool: &PgPool,
    batch_size: usize,
) -> anyhow::Result<Vec<EmbeddingJob>> {
    claim_embedding_jobs_matching(
        pool,
        batch_size,
        r#"
            e.status in ('pending', 'retry_wait') and e.next_attempt_at <= now()
        "#,
        "e.next_attempt_at, e.created_at",
    )
    .await
}

async fn claim_expired_embedding_jobs(
    pool: &PgPool,
    batch_size: usize,
) -> anyhow::Result<Vec<EmbeddingJob>> {
    claim_embedding_jobs_matching(
        pool,
        batch_size,
        r#"
            e.status = 'processing' and e.lease_expires_at <= now()
        "#,
        "e.lease_expires_at, e.created_at",
    )
    .await
}

async fn claim_embedding_jobs_matching(
    pool: &PgPool,
    batch_size: usize,
    predicate: &'static str,
    order_by: &'static str,
) -> anyhow::Result<Vec<EmbeddingJob>> {
    // Both fragments are private constants, never user-controlled SQL.
    let sql = format!(
        r#"
        with candidate as (
            select e.chat_id, e.message_id
            from telegram_message_embeddings e
            where ({predicate})
            order by {order_by}
            for update of e skip locked
            limit $1
        )
        update telegram_message_embeddings e
        set status = case when nullif(trim(m.text), '') is not null
                              and m.user_id is not null
                              and coalesce(p.is_bot, false) = false
                              and m.is_automatic_forward = false
                              and m.deleted_by_bot_at is null
                              and m.spam_marked_at is null
                          then 'processing' else 'ignored' end,
            attempts = case when nullif(trim(m.text), '') is not null
                                and m.user_id is not null
                                and coalesce(p.is_bot, false) = false
                                and m.is_automatic_forward = false
                                and m.deleted_by_bot_at is null
                                and m.spam_marked_at is null
                            then e.attempts + 1 else e.attempts end,
            processing_started_at = case when nullif(trim(m.text), '') is not null
                                         and m.user_id is not null
                                         and coalesce(p.is_bot, false) = false
                                         and m.is_automatic_forward = false
                                         and m.deleted_by_bot_at is null
                                         and m.spam_marked_at is null
                                     then now() else null end,
            lease_expires_at = case when nullif(trim(m.text), '') is not null
                                      and m.user_id is not null
                                      and coalesce(p.is_bot, false) = false
                                      and m.is_automatic_forward = false
                                      and m.deleted_by_bot_at is null
                                      and m.spam_marked_at is null
                                  then now() + ($2 * interval '1 second') else null end,
            updated_at = now()
        from candidate
        left join telegram_messages m on m.chat_id = candidate.chat_id and m.message_id = candidate.message_id
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where e.chat_id = candidate.chat_id and e.message_id = candidate.message_id
        returning e.status, e.chat_id, e.message_id, m.text, e.attempts
        "#
    );
    let rows = sqlx::query(&sql)
        .bind(batch_size.clamp(1, 64) as i64)
        .bind(LEASE_SECONDS)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            (row.get::<String, _>("status") == "processing").then(|| EmbeddingJob {
                chat_id: row.get("chat_id"),
                message_id: row.get("message_id"),
                text: row.get("text"),
                attempts: row.get("attempts"),
            })
        })
        .collect())
}

async fn mark_embedding_ready(
    pool: &PgPool,
    job: &EmbeddingJob,
    embedding: &[f32],
    model: &str,
) -> anyhow::Result<()> {
    let embedding = pgvector_literal(embedding)?;
    sqlx::query(
        r#"
        update telegram_message_embeddings e
        set embedding = case when m.text = $3 then $4::vector else null end,
            embedding_model = case when m.text = $3 then $5 else null end,
            status = case when m.text = $3 then 'ready' else 'pending' end,
            error_kind = null,
            next_attempt_at = case when m.text = $3 then e.next_attempt_at else now() end,
            lease_expires_at = null,
            updated_at = now()
        from telegram_messages m
        where e.chat_id = $1 and e.message_id = $2 and e.status = 'processing'
          and m.chat_id = e.chat_id and m.message_id = e.message_id
        "#,
    )
    .bind(job.chat_id)
    .bind(job.message_id)
    .bind(&job.text)
    .bind(embedding)
    .bind(model)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_embedding_failed(
    pool: &PgPool,
    job: &EmbeddingJob,
    error_kind: &str,
) -> anyhow::Result<()> {
    let (status, delay) = retry_after(job.attempts)
        .map(|delay| ("retry_wait", delay))
        .unwrap_or(("failed", 0));
    sqlx::query(
        r#"
        update telegram_message_embeddings
        set status = $3, error_kind = $4,
            next_attempt_at = now() + ($5 * interval '1 second'),
            lease_expires_at = null, updated_at = now()
        where chat_id = $1 and message_id = $2 and status = 'processing'
        "#,
    )
    .bind(job.chat_id)
    .bind(job.message_id)
    .bind(status)
    .bind(error_kind)
    .bind(delay)
    .execute(pool)
    .await?;
    Ok(())
}

fn retry_after(attempts: i32) -> Option<i64> {
    (attempts < MAX_RETRY_ATTEMPTS).then(|| 15 * 2_i64.pow(attempts.saturating_sub(1) as u32))
}

#[cfg(test)]
mod tests {
    use super::{
        BROAD_LITERAL_SCORE, PRECISE_LITERAL_SCORE, geometric_freshness, literal_match_score,
        literal_variants, retry_after, select_diverse_candidates,
    };
    use crate::features::chat_retrieval::RetrievalCandidate;

    #[test]
    fn retries_are_bounded_and_increase_geometrically() {
        assert_eq!(retry_after(1), Some(15));
        assert_eq!(retry_after(2), Some(30));
        assert_eq!(retry_after(3), Some(60));
        assert_eq!(retry_after(4), Some(120));
        assert_eq!(retry_after(5), None);
    }

    #[test]
    fn exact_terms_are_escaped_before_regex_search() {
        assert!(literal_variants("C++").iter().any(|term| term == "C\\+\\+"));
    }

    #[test]
    fn transliteration_covers_common_latin_product_names() {
        let variants = literal_variants("Windows Radeon");
        assert!(variants.iter().any(|term| term == "виндовс радеон"));
    }

    #[test]
    fn literal_score_requires_a_full_product_identifier_for_a_strong_boost() {
        assert_eq!(literal_match_score("RTX"), BROAD_LITERAL_SCORE);
        assert_eq!(literal_match_score("GIGABYTE"), BROAD_LITERAL_SCORE);
        assert_eq!(literal_match_score("RTX 5060"), PRECISE_LITERAL_SCORE);
        assert_eq!(literal_match_score("DDR5"), PRECISE_LITERAL_SCORE);
        assert_eq!(literal_match_score("C++"), PRECISE_LITERAL_SCORE);
    }

    #[test]
    fn diverse_selection_keeps_semantic_candidates_when_literal_matches_are_noisy() {
        let mut candidates = (1..=5)
            .map(|message_id| RetrievalCandidate {
                message_id,
                text: "generic literal match".to_string(),
                semantic_score: 0.0,
                lexical_score: 0.0,
                exact_score: BROAD_LITERAL_SCORE,
                freshness_score: 1.0,
                total_score: 1.2,
            })
            .collect::<Vec<_>>();
        candidates.push(RetrievalCandidate {
            message_id: 6,
            text: "semantic context".to_string(),
            semantic_score: 0.7,
            lexical_score: 0.0,
            exact_score: 0.0,
            freshness_score: 0.5,
            total_score: 1.2,
        });

        let selected = select_diverse_candidates(candidates, 12);
        assert!(selected.iter().any(|candidate| candidate.message_id == 6));
        assert_eq!(selected.len(), 5);
    }

    #[test]
    fn freshness_uses_configured_geometric_half_life() {
        assert_eq!(geometric_freshness(0.0, 7.0), 1.0);
        assert!((geometric_freshness(7.0, 7.0) - 0.5).abs() < 0.000_001);
        assert!((geometric_freshness(14.0, 7.0) - 0.25).abs() < 0.000_001);
    }
}
